//! Pack ranges, decode workers and bounded single-producer/single-consumer PCM queues.

use crate::{AudioError, Frame, Result, codec::AudioDecoder};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use symphonia::core::io::MediaSource;

/// A shared hard budget for queued PCM. Resident clips have their own explicit byte limit.
#[derive(Clone)]
pub struct StreamBudget {
    state: Arc<BudgetState>,
}
struct BudgetState {
    limit: usize,
    used: AtomicUsize,
}
struct Permit {
    state: Arc<BudgetState>,
    bytes: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.state.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
impl StreamBudget {
    /// Sets the aggregate streaming queue limit. Decoder scratch is documented separately.
    pub fn new(bytes: usize) -> Self {
        Self {
            state: Arc::new(BudgetState {
                limit: bytes,
                used: AtomicUsize::new(0),
            }),
        }
    }
    /// Bytes currently reserved by live queues, including a slot of conservative overhead.
    pub fn used_bytes(&self) -> usize {
        self.state.used.load(Ordering::Acquire)
    }
    fn reserve(&self, bytes: usize) -> Result<Permit> {
        self.state
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|n| *n <= self.state.limit)
            })
            .map_err(|_| AudioError::Capacity("stream memory budget"))?;
        Ok(Permit {
            state: Arc::clone(&self.state),
            bytes,
        })
    }
}

struct StreamState {
    eof: AtomicBool,
    failed: AtomicBool,
    underruns: AtomicU64,
    _permit: Permit,
}
/// Producer owned by a decode worker, never by an audio callback.
pub struct StreamWriter {
    queue: Producer<Frame>,
    state: Arc<StreamState>,
    sample_rate: u32,
}
/// Consumer moved into a voice. Reads never block or allocate; starvation emits silence.
pub struct StreamReader {
    queue: Consumer<Frame>,
    state: Arc<StreamState>,
    sample_rate: u32,
}

/// Allocates one bounded queue, charging it to the aggregate budget until both ends drop.
pub fn channel(
    sample_rate: u32,
    capacity_frames: usize,
    budget: &StreamBudget,
) -> Result<(StreamWriter, StreamReader)> {
    if !(8000..=192000).contains(&sample_rate) || !(4..=1920000).contains(&capacity_frames) {
        return Err(AudioError::Invalid("stream rate or capacity"));
    }
    let bytes = capacity_frames
        .checked_add(1)
        .and_then(|n| n.checked_mul(size_of::<Frame>()))
        .and_then(|n| n.checked_add(256))
        .ok_or(AudioError::Capacity("stream size"))?;
    let permit = budget.reserve(bytes)?;
    let (producer, consumer) = RingBuffer::new(capacity_frames);
    let state = Arc::new(StreamState {
        eof: AtomicBool::new(false),
        failed: AtomicBool::new(false),
        underruns: AtomicU64::new(0),
        _permit: permit,
    });
    Ok((
        StreamWriter {
            queue: producer,
            state: Arc::clone(&state),
            sample_rate,
        },
        StreamReader {
            queue: consumer,
            state,
            sample_rate,
        },
    ))
}
impl StreamWriter {
    /// True after EOF or after the voice dropped its consumer; loaders can release the decoder.
    pub fn is_finished(&self) -> bool {
        self.queue.is_abandoned() || self.state.eof.load(Ordering::Acquire)
    }
    /// Decodes only as many frames as can fit, preserving backpressure without dropping audio.
    pub fn fill(&mut self, decoder: &mut dyn AudioDecoder) -> Result<usize> {
        if self.queue.is_abandoned() {
            self.finish();
            return Ok(0);
        }
        if decoder.sample_rate() != self.sample_rate {
            return Err(AudioError::Invalid("stream sample rate changed"));
        }
        let mut frames = [Frame::ZERO; 1024];
        let limit = self.queue.slots().min(frames.len());
        if limit == 0 || self.state.eof.load(Ordering::Acquire) {
            return Ok(0);
        }
        let read = match decoder.read_frames(&mut frames[..limit]) {
            Ok(n) if n <= limit => n,
            Ok(_) => return Err(AudioError::Codec("decoder exceeded output buffer".into())),
            Err(error) => {
                self.state.failed.store(true, Ordering::Release);
                self.finish();
                return Err(error);
            }
        };
        if read == 0 {
            self.finish();
        }
        for &frame in &frames[..read] {
            self.queue
                .push(frame)
                .map_err(|_| AudioError::Capacity("stream queue invariant"))?;
        }
        Ok(read)
    }
    /// Marks a producer complete; the consumer drains queued frames before reporting EOF.
    pub fn finish(&mut self) {
        self.state.eof.store(true, Ordering::Release);
    }
}
impl Drop for StreamWriter {
    fn drop(&mut self) {
        self.finish();
    }
}
impl StreamReader {
    /// Source sample rate, before voice resampling.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// Number of frames ready for deterministic prebuffering.
    pub fn buffered_frames(&self) -> usize {
        self.queue.slots()
    }
    /// True after the decoder ended and every queued frame was consumed.
    pub fn ended(&self) -> bool {
        self.queue.is_empty() && self.state.eof.load(Ordering::Acquire)
    }
    /// Whether decoding failed rather than reaching normal EOF.
    pub fn failed(&self) -> bool {
        self.state.failed.load(Ordering::Acquire)
    }
    /// Number of missing frames substituted with silence before EOF.
    pub fn underruns(&self) -> u64 {
        self.state.underruns.load(Ordering::Relaxed)
    }
    pub(crate) fn next_frame(&mut self) -> Frame {
        match self.queue.pop() {
            Ok(frame) => frame,
            Err(_) => {
                if !self.state.eof.load(Ordering::Acquire) {
                    self.state.underruns.fetch_add(1, Ordering::Relaxed);
                }
                Frame::ZERO
            }
        }
    }
}

/// Worker lifetime guard. Drop/cancel it on a control worker, never on the render thread.
pub struct StreamWorker {
    cancel: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<Result<()>>>,
}
impl StreamWorker {
    /// Whether the decoder exited. Join finished workers on the control path to surface errors.
    pub fn is_finished(&self) -> bool {
        self.join
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }
    /// Stops further reads and joins the decoder, surfacing codec/IO errors.
    pub fn finish(mut self) -> Result<()> {
        self.cancel.store(true, Ordering::Release);
        match self.join.take() {
            Some(join) => join
                .join()
                .map_err(|_| AudioError::Backend("decoder worker panicked".into()))?,
            None => Ok(()),
        }
    }
}
impl Drop for StreamWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Prebuffers then starts a bounded decoder worker. Invoke on a loader worker under D20.
/// Offline goldens should fill channels synchronously instead of racing this worker.
pub fn spawn(
    mut decoder: Box<dyn AudioDecoder>,
    capacity_frames: usize,
    budget: &StreamBudget,
) -> Result<(StreamReader, StreamWorker)> {
    let (mut writer, reader) = channel(decoder.sample_rate(), capacity_frames, budget)?;
    while reader.buffered_frames() < capacity_frames && !reader.state.eof.load(Ordering::Acquire) {
        writer.fill(decoder.as_mut())?;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let join = thread::Builder::new()
        .name("ara-audio-decode".into())
        .spawn(move || {
            lower_worker_priority();
            while !worker_cancel.load(Ordering::Acquire)
                && !writer.state.eof.load(Ordering::Acquire)
            {
                if writer.fill(decoder.as_mut())? == 0 {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            writer.finish();
            Ok(())
        })?;
    Ok((
        reader,
        StreamWorker {
            cancel,
            join: Some(join),
        },
    ))
}

#[cfg(windows)]
fn lower_worker_priority() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
    }
    // SAFETY: changes only the current decoder worker to THREAD_PRIORITY_BELOW_NORMAL; no borrowed handles escape.
    unsafe {
        SetThreadPriority(GetCurrentThread(), -1);
    }
}
#[cfg(not(windows))]
fn lower_worker_priority() {}

/// A seekable media view confined to one asset's byte range inside a pack.
pub struct PackSlice<R> {
    inner: R,
    start: u64,
    len: u64,
    cursor: u64,
}
impl<R: Read + Seek> PackSlice<R> {
    /// Validates the range against the backing stream and seeks to its start.
    pub fn new(mut inner: R, start: u64, len: u64) -> Result<Self> {
        let end = start
            .checked_add(len)
            .ok_or(AudioError::Invalid("pack range overflow"))?;
        if len == 0 || end > inner.seek(SeekFrom::End(0))? {
            return Err(AudioError::Invalid("pack range outside source"));
        }
        inner.seek(SeekFrom::Start(start))?;
        Ok(Self {
            inner,
            start,
            len,
            cursor: 0,
        })
    }
}
impl<R: Read + Seek> Read for PackSlice<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let len = out
            .len()
            .min((self.len - self.cursor).min(usize::MAX as u64) as usize);
        let n = self.inner.read(&mut out[..len])?;
        self.cursor += n as u64;
        Ok(n)
    }
}
impl<R: Read + Seek> Seek for PackSlice<R> {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.len) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.cursor) + i128::from(n),
        };
        if position < 0 || position > i128::from(self.len) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek outside pack asset",
            ));
        }
        self.inner
            .seek(SeekFrom::Start(self.start + position as u64))?;
        self.cursor = position as u64;
        Ok(self.cursor)
    }
}
impl<R: Read + Seek + Send + Sync> MediaSource for PackSlice<R> {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}
