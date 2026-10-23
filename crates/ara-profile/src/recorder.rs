use crate::{
    allocator::AllocationSample,
    ring::{Control, PRODUCER, Producer, Ring},
    *,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
static CAPTURE_ID: AtomicU64 = AtomicU64::new(1);

/// Bounded capture storage, allocated before measurement starts.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Rolling frames retained; must be nonzero.
    pub history: usize,
    /// CPU records retained per frame across all threads.
    pub spans_per_frame: usize,
    /// Records queued per thread; must be a nonzero power of two.
    pub ring_capacity: usize,
    /// Maximum GPU passes retained per frame.
    pub gpu_passes: usize,
    /// CPU frame budget for hitch detection.
    pub budget_ns: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            history: 120,
            spans_per_frame: 2048,
            ring_capacity: 8192,
            gpu_passes: 128,
            budget_ns: 16_666_667,
        }
    }
}

/// Single-consumer owner of frame history. Queries and file IO stay off tick workers.
pub struct Profiler {
    control: Arc<Control>,
    config: Config,
    rings: Vec<Arc<Ring>>,
    profile: Profile,
    spare: VecDeque<Frame>,
    labels: BTreeMap<&'static str, u32>,
    dynamic_labels: BTreeMap<u32, u32>,
    external_labels: BTreeMap<String, u32>,
    next_frame: u64,
    open: Option<(u64, AllocationSample)>,
}

/// Install once on a worker before its hot loop. Each registration has one producer.
pub struct ThreadRegistration {
    control: Arc<Control>,
    ring: Arc<Ring>,
}
impl ThreadRegistration {
    /// Attaches this registration to the calling thread. Refuses to replace a producer.
    pub fn attach(self) -> Result<(), &'static str> {
        PRODUCER.with_borrow_mut(|p| {
            if p.is_some() {
                return Err("profiling thread is already attached");
            }
            *p = Some(Producer::new(self.control, self.ring));
            Ok(())
        })
    }
    /// Detaches after all scopes have exited, allowing another capture on this thread.
    pub fn detach() {
        PRODUCER.with_borrow_mut(|p| *p = None);
    }
}

/// Immutable originating-frame identity. Move to a registered worker or serialize
/// its ID with an out-of-process job, then submit that worker's trace separately.
#[derive(Debug, Clone, Copy)]
pub struct FrameToken {
    frame: u64,
    owner: u64,
}
impl FrameToken {
    /// Frame correlation ID; zero is never issued.
    pub fn id(self) -> u64 {
        self.frame
    }
    /// Records work against the originating frame, restoring the worker's context
    /// even during unwinding. Tokens belong to their issuing profiler.
    pub fn in_scope<R>(self, run: impl FnOnce() -> R) -> Result<R, &'static str> {
        struct Reset(Option<u64>);
        impl Drop for Reset {
            fn drop(&mut self) {
                PRODUCER.with_borrow_mut(|p| {
                    if let Some(p) = p {
                        p.frame_override = self.0;
                    }
                });
            }
        }
        let previous = PRODUCER.with_borrow_mut(|p| {
            let p = p.as_mut().ok_or("worker thread is not attached")?;
            if p.control.id != self.owner {
                return Err("frame token belongs to another profiler");
            }
            Ok(p.frame_override.replace(self.frame))
        })?;
        let reset = Reset(previous);
        let result = run();
        drop(reset);
        Ok(result)
    }
}

impl Profiler {
    /// Allocates the bounded rolling store. No threads, allocator or subscriber
    /// are installed implicitly. Configuration is validated before allocating.
    pub fn new(config: Config) -> Result<Self, &'static str> {
        if config.history == 0
            || config.history > 10_000
            || config.spans_per_frame == 0
            || config.spans_per_frame > 1_000_000
            || !config.ring_capacity.is_power_of_two()
            || config.ring_capacity > 1_000_000
            || config.gpu_passes > 65_536
            || config.budget_ns == 0
        {
            return Err("invalid profiling capacity or frame budget");
        }
        let spare = (0..config.history)
            .map(|_| Frame {
                spans: Vec::with_capacity(config.spans_per_frame),
                gpu: Vec::with_capacity(config.gpu_passes),
                jobs: Vec::with_capacity(config.gpu_passes),
                ..Frame::default()
            })
            .collect();
        Ok(Self {
            control: Arc::new(Control {
                id: CAPTURE_ID.fetch_add(1, Ordering::Relaxed),
                names: std::sync::Mutex::default(),
                enabled: AtomicBool::new(false),
                frame: AtomicU64::new(0),
                origin: Instant::now(),
            }),
            config,
            rings: Vec::new(),
            profile: Profile {
                version: 1,
                budget_ns: config.budget_ns,
                frames: VecDeque::with_capacity(config.history),
                ..Profile::default()
            },
            spare,
            labels: BTreeMap::new(),
            dynamic_labels: BTreeMap::new(),
            external_labels: BTreeMap::new(),
            next_frame: 0,
            open: None,
        })
    }
    /// Creates one thread endpoint. Call during setup, then move it to that worker.
    pub fn register_thread(&mut self, name: impl Into<String>) -> ThreadRegistration {
        let ring = Arc::new(Ring::new(self.config.ring_capacity));
        self.profile.threads.push(ThreadInfo {
            id: self.rings.len() as u32,
            name: name.into(),
        });
        self.rings.push(Arc::clone(&ring));
        ThreadRegistration {
            control: Arc::clone(&self.control),
            ring,
        }
    }
    /// Starts recording without discarding the rolling history.
    pub fn start(&self) {
        #[cfg(feature = "tracy")]
        tracy_client::Client::start();
        if !self.control.enabled.swap(true, Ordering::AcqRel) {
            crate::ACTIVE_CAPTURES.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Stops new scopes; outstanding scopes can still finish and be collected.
    pub fn stop(&self) {
        if self.control.enabled.swap(false, Ordering::AcqRel) {
            crate::ACTIVE_CAPTURES.fetch_sub(1, Ordering::Relaxed);
        }
    }
    /// Whether new scopes are being recorded.
    pub fn is_recording(&self) -> bool {
        self.control.enabled.load(Ordering::Acquire)
    }
    /// Whether this thread already has an endpoint for this capture.
    pub fn current_thread_attached(&self) -> bool {
        PRODUCER.with_borrow(|p| p.as_ref().is_some_and(|p| p.control.id == self.control.id))
    }
    /// Begins a CPU frame. A previous frame must be ended explicitly.
    pub fn begin_frame(&mut self) -> Result<FrameToken, &'static str> {
        if self.open.is_some() {
            return Err("profiling frame already open");
        }
        self.next_frame = self.next_frame.checked_add(1).ok_or("frame ID exhausted")?;
        let mut frame = if let Some(frame) = self.spare.pop_front() {
            frame
        } else {
            self.profile
                .frames
                .pop_front()
                .ok_or("missing preallocated frame")?
        };
        frame.spans.clear();
        frame.gpu.clear();
        frame.jobs.clear();
        frame.id = self.next_frame;
        frame.start_ns = self.control.now();
        frame.duration_ns = 0;
        frame.dropped = 0;
        frame.counters = Counters::default();
        frame.memory = Memory::default();
        frame.load = BackgroundLoad::default();
        self.profile.frames.push_back(frame);
        self.open = Some((self.next_frame, AllocationSample::now()));
        self.control.frame.store(self.next_frame, Ordering::Release);
        Ok(FrameToken {
            frame: self.next_frame,
            owner: self.control.id,
        })
    }
    /// Ends the frame and drains published records without waiting for workers.
    /// Reuses frame storage; after static names are interned this path allocates nothing.
    pub fn end_frame(
        &mut self,
        counters: Counters,
        mut memory: Memory,
    ) -> Result<(), &'static str> {
        let (id, before) = self.open.take().ok_or("no profiling frame open")?;
        self.control.frame.store(0, Ordering::Release);
        let now = self.control.now();
        let after = AllocationSample::now();
        memory.cpu_heap = heap_bytes();
        memory.allocated_bytes = after.bytes.wrapping_sub(before.bytes);
        memory.allocations = after.calls.wrapping_sub(before.calls);
        let frame = self
            .profile
            .frames
            .back_mut()
            .filter(|f| f.id == id)
            .ok_or("open frame was evicted")?;
        frame.duration_ns = now.saturating_sub(frame.start_ns);
        frame.counters = counters;
        frame.memory = memory;
        self.collect();
        #[cfg(feature = "tracy")]
        if let Some(client) = tracy_client::Client::running() {
            client.frame_mark();
        }
        Ok(())
    }
    /// Drains completed worker scopes into their original frames, including after stop.
    pub fn collect(&mut self) {
        for (thread, ring) in self.rings.iter().enumerate() {
            while let Some(raw) = ring.pop() {
                let base = self.profile.frames.front().map_or(0, |f| f.id);
                let Some(frame) = raw
                    .frame
                    .checked_sub(base)
                    .and_then(|index| self.profile.frames.get_mut(index as usize))
                else {
                    self.profile.orphaned_results += 1;
                    continue;
                };
                if frame.spans.len() == self.config.spans_per_frame {
                    frame.dropped += 1;
                    continue;
                }
                let label = match raw.name {
                    crate::ring::Name::Static(name) => {
                        *self.labels.entry(name).or_insert_with(|| {
                            let id = self.profile.labels.len() as u32;
                            self.profile.labels.push(name.into());
                            id
                        })
                    }
                    crate::ring::Name::Registered(index) => {
                        *self.dynamic_labels.entry(index).or_insert_with(|| {
                            let names = self
                                .control
                                .names
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            let id = self.profile.labels.len() as u32;
                            self.profile.labels.push(
                                names
                                    .values
                                    .get(index as usize)
                                    .map_or_else(|| "<invalid label>".into(), String::to_owned),
                            );
                            id
                        })
                    }
                };
                frame.spans.push(CpuSpan {
                    label,
                    thread: thread as u32,
                    id: raw.id,
                    parent: raw.parent,
                    start_ns: raw.start,
                    duration_ns: raw.duration,
                    self_ns: raw.self_ns,
                    allocated_bytes: raw.allocated,
                    self_allocated_bytes: raw.self_allocated,
                });
            }
            if let Some(frame) = self.profile.frames.back_mut() {
                frame.dropped += ring.dropped.swap(0, Ordering::Relaxed);
            }
        }
    }
    /// Attaches a GPU result by its submission frame, rejecting invalid/evicted results.
    pub fn gpu_pass(&mut self, frame: u64, name: &str, queue: &str, nanoseconds: u64) -> bool {
        let Some(index) = self.frame_index(frame) else {
            self.profile.orphaned_results += 1;
            return false;
        };
        let label = self.intern_external(name);
        let queue = self.intern_external(queue);
        let f = &mut self.profile.frames[index];
        if f.gpu.len() >= self.config.gpu_passes {
            f.dropped += 1;
            return false;
        }
        f.gpu.push(GpuPass {
            label,
            queue,
            duration_ns: nanoseconds,
        });
        true
    }
    fn frame_index(&self, frame: u64) -> Option<usize> {
        let base = self.profile.frames.front()?.id;
        let index = usize::try_from(frame.checked_sub(base)?).ok()?;
        (index < self.profile.frames.len()).then_some(index)
    }
    fn intern_external(&mut self, name: &str) -> u32 {
        if let Some(label) = self.external_labels.get(name) {
            return *label;
        }
        let label = self.profile.labels.len() as u32;
        self.profile.labels.push(name.into());
        self.external_labels.insert(name.into(), label);
        label
    }
    /// Job-manager seam for assets, navigation and compile-server processes.
    /// Report worker elapsed durations without pretending clocks are synchronized.
    pub fn job_activity(
        &mut self,
        frame: u64,
        category: &str,
        worker: u64,
        duration_ns: u64,
        complete: bool,
    ) -> bool {
        let Some(index) = self.frame_index(frame) else {
            self.profile.orphaned_results += 1;
            return false;
        };
        let label = self.intern_external(category);
        let f = &mut self.profile.frames[index];
        if f.jobs.len() >= self.config.gpu_passes {
            f.dropped += 1;
            return false;
        }
        f.jobs.push(JobActivity {
            label,
            worker,
            duration_ns,
            complete,
        });
        true
    }
    /// Imports ara-gpu's resolved device timestamps; pass the submitting frame ID,
    /// never the CPU frame in which a readback happens to become available.
    #[cfg(feature = "gpu")]
    pub fn gpu_timings(&mut self, frame: u64, timings: &[ara_gpu::commands::PassTiming]) {
        for timing in timings {
            if timing.milliseconds.is_finite()
                && timing.milliseconds >= 0.0
                && timing.milliseconds * 1_000_000.0 < u64::MAX as f64
            {
                let queue = match timing.queue {
                    ara_gpu::Queue::Graphics => "graphics",
                    ara_gpu::Queue::Compute => "compute",
                };
                self.gpu_pass(
                    frame,
                    &timing.pass,
                    queue,
                    (timing.milliseconds * 1_000_000.0) as u64,
                );
            } else if let Some(index) = self.frame_index(frame) {
                self.profile.frames[index].dropped += 1;
            } else {
                self.profile.orphaned_results += 1;
            }
        }
    }
    /// Editor/agent data seam; borrowed timeline, flame edges, GPU list and counters.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    /// Records an OS load sample on a retained frame.
    pub fn set_load(&mut self, id: u64, load: BackgroundLoad) -> bool {
        if let Some(frame) = self.profile.frames.iter_mut().find(|f| f.id == id) {
            frame.load = load;
            true
        } else {
            false
        }
    }
}

impl Drop for Profiler {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Enters a native CPU scope on an explicitly attached thread; inactive without
/// recording/a frame. `ProfileLayer` routes ordinary tracing spans to the same ring.
#[inline(always)]
pub fn scope(name: &'static str) -> SpanGuard {
    if crate::instrumentation_enabled() {
        SpanGuard::new(name)
    } else {
        SpanGuard::inactive()
    }
}
