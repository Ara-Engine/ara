//! The automatic-exposure meter's look at a frame, off the frame's critical path. A compute
//! pass on Renderlab's own device reduces the engine's output to a log-luminance histogram
//! (`BINS` bins); the histogram is copied to a staging buffer and mapped on a later frame,
//! so no frame waits for the GPU or for a large readback. The bridge polls at the start of
//! each frame, when the previous one (which it waited for) and the pass queued behind it are
//! done, and `Metering::observe` takes what finished: a reading is one frame old, as the
//! blocking read's was, and a slower GPU only makes it later, which the meter's own
//! adaptation (Ara's rate of 2 per second) hides. A pass that never finishes is reported
//! and given up on (`STUCK_FRAMES`); its slot stays busy until it does.
use crate::gaps::Gaps;
use std::{
    collections::VecDeque,
    sync::{Arc, OnceLock},
};

/// Histogram bins over the log of display luminance, `LOG_DARKEST` to 0 (white): about
/// 0.026 stops a bin, finer than the meter's 0.15-stop dead band.
pub(crate) const BINS: usize = 512;
/// The log of the darkest luminance the meter tells apart (1e-4, as Ara's meter floors it).
pub(crate) const LOG_DARKEST: f32 = -9.210_34;
/// Pixels between samples on each axis: a quarter of the frame is binned.
const STRIDE: u32 = 2;
/// Readback slots made at start; usually one or two are in flight.
const SLOTS: usize = 3;
/// The most slots ever made, stuck ones included.
const MAX_SLOTS: usize = 8;
/// Frames a pass may stay in flight before it is called stuck (a pass finishes within the
/// frame it follows, which the bridge waits for, so one or two is normal).
const STUCK_FRAMES: u32 = 8;
/// Consecutive refused submits before it is reported.
const REFUSED_FRAMES: u32 = 60;

/// The reduction: each invocation bins one sampled pixel's luminance (the output is sRGB, so
/// the load decodes to linear light).
fn shader() -> String {
    format!(
        "@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bins: array<atomic<u32>, {BINS}>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let at = id.xy * {STRIDE}u;
    let size = textureDimensions(image);
    if (at.x >= size.x || at.y >= size.y) {{ return; }}
    let c = textureLoad(image, vec2<i32>(at), 0).rgb;
    let l = max(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-4);
    let x = (log(l) - ({LOG_DARKEST:?})) / (0.0 - ({LOG_DARKEST:?}));
    atomicAdd(&bins[u32(clamp(floor(x * {BINS}.0), 0.0, {BINS}.0 - 1.0))], 1u);
}}
"
    )
}

/// The bin of luminance `l` (0 to 1), as the shader bins it.
#[cfg(test)]
pub(crate) fn bin_of(l: f32) -> usize {
    let x = (l.max(1e-4).ln() - LOG_DARKEST) / -LOG_DARKEST;
    ((x * BINS as f32).floor().clamp(0., BINS as f32 - 1.)) as usize
}

/// The histogram the shader would build for `rgba8` (display-encoded, `width` pixels wide):
/// the same sampled pixels and bins, on the CPU. For tests.
#[cfg(test)]
pub(crate) fn histogram_of_rgba8(rgba8: &[u8], width: usize) -> Vec<u32> {
    let decode = |v: u8| {
        let s = f32::from(v) / 255.;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut bins = vec![0u32; BINS];
    for (i, p) in rgba8.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (i % width, i / width);
        if x % STRIDE as usize != 0 || y % STRIDE as usize != 0 {
            continue;
        }
        let l = 0.2126 * decode(p[0]) + 0.7152 * decode(p[1]) + 0.0722 * decode(p[2]);
        bins[bin_of(l)] += 1;
    }
    bins
}

/// The mean of the log luminance of the samples between the `low` and `high` percentiles
/// (0 to 1) of `histogram`, each bin at its centre, a bin cut by a percentile counted in
/// part. None for an empty histogram.
pub(crate) fn log_mean(histogram: &[u32], low: f32, high: f32) -> Option<f32> {
    let total: f32 = histogram.iter().map(|&c| c as f32).sum();
    if total <= 0. {
        return None;
    }
    let first = total * low.clamp(0., 1.);
    // At least one sample between the percentiles.
    let last = (total * high.clamp(0., 1.)).max(first + 1.).min(total);
    let (mut seen, mut sum, mut weight) = (0f32, 0f32, 0f32);
    for (bin, &count) in histogram.iter().enumerate() {
        let count = count as f32;
        let inside = ((seen + count).min(last) - seen.max(first)).max(0.);
        let centre = LOG_DARKEST * (1. - (bin as f32 + 0.5) / BINS as f32);
        sum += inside * centre;
        weight += inside;
        seen += count;
    }
    (weight > 0.).then(|| sum / weight)
}

/// One finished look at a frame: the meter and stops it was rendered with, and its histogram.
pub(crate) struct Reading {
    pub meter: f32,
    pub stops: f32,
    pub histogram: Vec<u32>,
}

type Mapped = Arc<OnceLock<Result<(), wgpu::BufferAsyncError>>>;

struct Pending {
    meter: f32,
    stops: f32,
    epoch: u32,
    mapped: Mapped,
    /// `poll`s since the pass was queued: frames in flight.
    age: u32,
    /// Reported as stuck: later readings no longer wait for it.
    stuck: bool,
}

struct Slot {
    bins: wgpu::Buffer,
    staging: wgpu::Buffer,
    pending: Option<Pending>,
}

struct Pipeline {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

/// The histogram passes in flight on one device. The pipeline and the first `SLOTS` buffer
/// pairs are made at construction, so the first metered frame pays for neither.
pub(crate) struct Readbacks {
    pipeline: Pipeline,
    slots: Vec<Slot>,
    /// Slot indices in submission order.
    flight: VecDeque<usize>,
    /// Bumped by `discard`: readings of an earlier epoch are dropped when they arrive.
    epoch: u32,
    /// Consecutive `submit`s refused for want of a slot.
    refused: u32,
}

impl Readbacks {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            pipeline: Pipeline::new(device),
            slots: (0..SLOTS).map(|_| Slot::new(device)).collect(),
            flight: VecDeque::new(),
            epoch: 0,
            refused: 0,
        }
    }

    /// Queues a histogram pass over `output` (display-encoded RGBA8, `TEXTURE_BINDING`) after
    /// the frame already submitted to `queue`, remembering the `meter` and `stops` it shows.
    /// When every slot is in flight it makes another (a buffer with a map outstanding cannot
    /// be reused), up to `MAX_SLOTS`; past that it returns false, and the caller tries again
    /// next frame. A long run of refusals is reported once.
    pub fn submit(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        output: &wgpu::Texture,
        meter: f32,
        stops: f32,
        gaps: &mut Gaps,
    ) -> bool {
        let index = match self.slots.iter().position(|s| s.pending.is_none()) {
            Some(index) => index,
            None if self.slots.len() < MAX_SLOTS => {
                self.slots.push(Slot::new(device));
                self.slots.len() - 1
            }
            None => {
                self.refused += 1;
                if self.refused == REFUSED_FRAMES {
                    gaps.fault(
                        "exposure meter readbacks refused",
                        &format!(
                            "all {MAX_SLOTS} readback slots have been busy for {REFUSED_FRAMES} frames: the GPU is not finishing the meter's passes, and the meter keeps its last target"
                        ),
                    );
                }
                return false;
            }
        };
        self.refused = 0;
        let (gpu, Some(slot)) = (&self.pipeline, self.slots.get_mut(index)) else {
            return false;
        };
        let view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("exposure meter"),
            layout: &gpu.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: slot.bins.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.clear_buffer(&slot.bins, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &group, &[]);
            let samples = |extent: u32| extent.div_ceil(STRIDE).div_ceil(8);
            pass.dispatch_workgroups(samples(output.width()), samples(output.height()), 1);
        }
        encoder.copy_buffer_to_buffer(&slot.bins, 0, &slot.staging, 0, (BINS * 4) as u64);
        let mapped: Mapped = Arc::new(OnceLock::new());
        let done = Arc::clone(&mapped);
        encoder.map_buffer_on_submit(&slot.staging, wgpu::MapMode::Read, .., move |result| {
            // Set once per submission; a second set cannot happen.
            let _ = done.set(result);
        });
        queue.submit([encoder.finish()]);
        slot.pending = Some(Pending {
            meter,
            stops,
            epoch: self.epoch,
            mapped,
            age: 0,
            stuck: false,
        });
        self.flight.push_back(index);
        true
    }

    /// The readings whose passes have finished since the last call, oldest first, without
    /// waiting for any other. Readings of a `discard`ed epoch, failed maps and readings more
    /// than `STUCK_FRAMES` old are dropped. A pass still unfinished after `STUCK_FRAMES`
    /// polls is reported once and no longer holds back the ones behind it; its slot stays
    /// busy until its map completes, and `submit` makes others meanwhile.
    pub fn poll(&mut self, device: &wgpu::Device, gaps: &mut Gaps) -> Vec<Reading> {
        if self.flight.is_empty() {
            return Vec::new();
        }
        // Processes map callbacks; never waits.
        if let Err(error) = device.poll(wgpu::PollType::Poll) {
            gaps.fault(
                "exposure meter poll",
                &format!("{error}: readings may stop arriving"),
            );
        }
        let mut readings = Vec::new();
        let mut waiting = VecDeque::new();
        // An older pass that is not finished (and not stuck) holds back the newer ones.
        let mut held = false;
        for index in std::mem::take(&mut self.flight) {
            let Some(slot) = self.slots.get_mut(index) else {
                continue;
            };
            let Some(pending) = slot.pending.as_mut() else {
                continue;
            };
            pending.age += 1;
            let (meter, stops, epoch, age) =
                (pending.meter, pending.stops, pending.epoch, pending.age);
            let outcome = pending.mapped.get().map(|result| {
                result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });
            let Some(outcome) = outcome else {
                if age > STUCK_FRAMES {
                    if !pending.stuck {
                        pending.stuck = true;
                        gaps.fault(
                            "exposure meter readback never completed",
                            &format!(
                                "a histogram pass has been in flight for {age} frames: later readings go ahead of it and its slot stays busy until it completes"
                            ),
                        );
                    }
                } else {
                    held = true;
                }
                waiting.push_back(index);
                continue;
            };
            if held {
                waiting.push_back(index);
                continue;
            }
            match outcome {
                Ok(()) => {
                    let fresh = epoch == self.epoch && age <= STUCK_FRAMES;
                    if let Some(histogram) = slot.read().filter(|_| fresh) {
                        readings.push(Reading {
                            meter,
                            stops,
                            histogram,
                        });
                    }
                    slot.staging.unmap();
                }
                Err(error) => gaps.fault(
                    "exposure meter readback failed",
                    &format!("{error}: that reading is dropped"),
                ),
            }
            slot.pending = None;
        }
        self.flight = waiting;
        readings
    }

    /// After a camera cut: readings of frames submitted so far describe the old view, so
    /// they are dropped as they arrive.
    pub fn discard(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }
}

impl Pipeline {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("exposure meter"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("exposure meter"),
            source: wgpu::ShaderSource::Wgsl(shader().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("exposure meter"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("exposure meter"),
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                }),
            ),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }
}

impl Slot {
    fn new(device: &wgpu::Device) -> Self {
        let size = (BINS * 4) as u64;
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            bins: buffer(
                "exposure meter bins",
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            staging: buffer(
                "exposure meter readback",
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            pending: None,
        }
    }

    /// The mapped histogram, or None if the range cannot be read.
    fn read(&self) -> Option<Vec<u32>> {
        let mapped = self.staging.get_mapped_range(..).ok()?;
        Some(
            mapped
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_ne_bytes(*b))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a future to completion on this thread.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        struct Unpark(std::thread::Thread);
        impl std::task::Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Arc::new(Unpark(std::thread::current())).into();
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::thread::park(),
            }
        }
    }

    /// The percentiles trim the histogram: a bright 10% is left out below the 90th.
    #[test]
    fn the_log_mean_leaves_out_the_trimmed_tails() {
        let mut histogram = vec![0u32; BINS];
        histogram[bin_of(0.18)] = 90;
        histogram[bin_of(0.9)] = 10;
        let width = -LOG_DARKEST / BINS as f32;
        let mid = log_mean(&histogram, 0., 0.9).expect("samples");
        assert!((mid - 0.18f32.ln()).abs() < width, "{mid}");
        let all = log_mean(&histogram, 0., 1.).expect("samples");
        let expected = 0.9 * 0.18f32.ln() + 0.1 * 0.9f32.ln();
        assert!((all - expected).abs() < width, "{all} vs {expected}");
        assert_eq!(log_mean(&vec![0; BINS], 0., 1.), None);
    }

    /// The shader and its CPU twin bin the same frame the same way: a frame of flat blocks of
    /// known greys (each far from a bin edge) through a real device lands in the same bins,
    /// arrives without waiting, and a discard drops it.
    #[test]
    fn the_gpu_histogram_matches_its_cpu_twin_and_arrives_without_a_wait() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let Ok(adapter) = block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no adapter: the GPU histogram was not checked");
            return;
        };
        let (device, queue) =
            block_on(adapter.request_device(&Default::default())).expect("device");
        // 100x80: five vertical stripes of different greys (and a width that is not a multiple
        // of the workgroup or the stride, to cover the bounds check).
        let (w, h) = (100u32, 80u32);
        let greys = [4u8, 30, 90, 160, 250];
        let rgba8: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let g = greys[((i % w) / 20) as usize];
                [g, g, g, 255]
            })
            .collect();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("meter test"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &rgba8,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: None,
            },
            texture.size(),
        );
        let mut gaps = Gaps::default();
        let mut readbacks = Readbacks::new(&device);
        assert!(readbacks.submit(&device, &queue, &texture, -2.5, -1.5, &mut gaps));
        // Polling never waits: right after the submit the reading may or may not be there.
        let early = readbacks.poll(&device, &mut gaps);
        // One frame on, the pass is done (the bridge waits for each frame, and the pass is
        // queued right behind it) and the reading arrives.
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mut readings = early;
        readings.extend(readbacks.poll(&device, &mut gaps));
        assert_eq!(readings.len(), 1, "one reading, exactly once");
        let reading = &readings[0];
        assert_eq!((reading.meter, reading.stops), (-2.5, -1.5));
        let twin = histogram_of_rgba8(&rgba8, w as usize);
        assert_eq!(reading.histogram.iter().sum::<u32>(), 50 * 40);
        assert_eq!(reading.histogram, twin);
        // A reading of a frame submitted before a cut is dropped on arrival.
        assert!(readbacks.submit(&device, &queue, &texture, 0., 0., &mut gaps));
        readbacks.discard();
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        assert!(readbacks.poll(&device, &mut gaps).is_empty());
        assert!(readbacks.flight.is_empty(), "the slot is free again");
    }

    /// A device and a 4x4 texture to meter, or None without an adapter.
    fn gpu() -> Option<(wgpu::Device, wgpu::Queue, wgpu::Texture)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) = block_on(adapter.request_device(&Default::default())).ok()?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("meter test"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Some((device, queue, texture))
    }

    /// Marks `slot` as holding a pass whose map never completes.
    fn jam(readbacks: &mut Readbacks, slot: usize) {
        readbacks.slots[slot].pending = Some(Pending {
            meter: 0.,
            stops: 0.,
            epoch: readbacks.epoch,
            mapped: Arc::new(OnceLock::new()),
            age: 0,
            stuck: false,
        });
        readbacks.flight.push_back(slot);
    }

    /// A readback that never completes is counted, reported once, and no longer holds the
    /// meter: past `STUCK_FRAMES` polls the next pass gets a new slot and its reading arrives.
    #[test]
    fn a_stuck_readback_is_reported_and_does_not_stop_the_meter() {
        let Some((device, queue, texture)) = gpu() else {
            eprintln!("no adapter: stuck readbacks were not checked");
            return;
        };
        let (mut gaps, mut readbacks) = (Gaps::default(), Readbacks::new(&device));
        // Every slot made at start is jammed.
        for slot in 0..SLOTS {
            jam(&mut readbacks, slot);
        }
        assert!(!gaps.reported("exposure meter readback never completed"));
        for _ in 0..=STUCK_FRAMES {
            assert!(readbacks.poll(&device, &mut gaps).is_empty());
        }
        assert!(gaps.reported("exposure meter readback never completed"));
        // A new slot takes the next pass, and its reading is not held behind the jammed ones.
        assert!(readbacks.submit(&device, &queue, &texture, 1., 2., &mut gaps));
        assert_eq!(readbacks.slots.len(), SLOTS + 1);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let readings = readbacks.poll(&device, &mut gaps);
        assert_eq!(
            readings.len(),
            1,
            "the new slot's reading, not the jammed ones'"
        );
        assert_eq!((readings[0].meter, readings[0].stops), (1., 2.));
    }

    /// With every slot jammed up to the limit, a submit is refused (the bridge asks again
    /// next frame), and a long run of refusals is reported once.
    #[test]
    fn refused_submits_are_reported_after_a_long_run() {
        let Some((device, queue, texture)) = gpu() else {
            eprintln!("no adapter: refused submits were not checked");
            return;
        };
        let (mut gaps, mut readbacks) = (Gaps::default(), Readbacks::new(&device));
        for slot in 0..MAX_SLOTS {
            if slot >= readbacks.slots.len() {
                readbacks.slots.push(Slot::new(&device));
            }
            jam(&mut readbacks, slot);
        }
        for _ in 0..REFUSED_FRAMES - 1 {
            assert!(!readbacks.submit(&device, &queue, &texture, 0., 0., &mut gaps));
        }
        assert!(!gaps.reported("exposure meter readbacks refused"));
        assert!(!readbacks.submit(&device, &queue, &texture, 0., 0., &mut gaps));
        assert!(gaps.reported("exposure meter readbacks refused"));
    }

    /// A cut's own reading is the next one applied: with every slot busy on passes from before
    /// the cut, `discard` drops those, the cut frame's pass still gets a slot (a new one), and
    /// the only reading that comes back is the cut frame's.
    #[test]
    fn a_cut_frames_reading_is_the_next_one_applied() {
        let Some((device, queue, texture)) = gpu() else {
            eprintln!("no adapter: the cut bound was not checked");
            return;
        };
        let (mut gaps, mut readbacks) = (Gaps::default(), Readbacks::new(&device));
        for old in 0..SLOTS {
            assert!(readbacks.submit(&device, &queue, &texture, old as f32, 0., &mut gaps));
        }
        assert!(
            readbacks.slots.iter().all(|s| s.pending.is_some()),
            "all busy"
        );
        // The cut: readings in flight describe the old view.
        readbacks.discard();
        assert!(
            readbacks.submit(&device, &queue, &texture, 99., 0., &mut gaps),
            "the cut frame's pass cannot be refused"
        );
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let readings = readbacks.poll(&device, &mut gaps);
        assert_eq!(readings.len(), 1, "only the cut frame's reading is applied");
        assert_eq!(readings[0].meter, 99.);
    }
}
