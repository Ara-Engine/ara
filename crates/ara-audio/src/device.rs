//! SDL3 streams and deterministic offline output behind the same sink contract.
//! SDL owns the hardware callback; Ara feeds it from a dedicated presentation worker.

use crate::{AudioEngine, AudioError, QUANTUM, Result};
use libloading::Library;
use sdl3_sys::{
    audio::{
        SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK, SDL_AUDIO_F32, SDL_AudioSpec, SDL_AudioStream,
        SDL_DestroyAudioStream, SDL_GetAudioDeviceFormat, SDL_GetAudioDeviceName,
        SDL_GetAudioStreamDevice, SDL_GetAudioStreamQueued, SDL_GetCurrentAudioDriver,
        SDL_OpenAudioDeviceStream, SDL_PutAudioStreamData, SDL_ResumeAudioStreamDevice,
    },
    error::SDL_GetError,
    hints::SDL_SetHint,
    init::{SDL_INIT_AUDIO, SDL_InitSubSystem, SDL_QuitSubSystem},
};
use std::{
    ffi::{CStr, c_int},
    ptr,
};

/// Hardware format, used for device change detection and buffer latency reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceFormat {
    /// Device frames per second.
    pub sample_rate: u32,
    /// Device channel count.
    pub channels: u16,
    /// SDL-reported hardware callback buffer length in frames.
    pub buffer_frames: u32,
}
/// Device state change, forwarded to diagnostics by the platform bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEvent {
    /// Output is accepting data, with no detected format change.
    Ready,
    /// Hardware output format changed; SDL follows default-device migrations internally.
    Changed,
    /// Output was lost. Call recover on the presentation worker and keep the last good graph.
    Lost,
    /// Explicit recovery reopened the default output without restarting voices.
    Recovered,
}
/// Estimated output pipeline latency; this is not an acoustic round-trip measurement.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Latency {
    /// Input frames currently queued in the backend, in engine-rate frames.
    pub queued_frames: usize,
    /// Known queue + device period duration; hidden processing makes this a lower-bound estimate.
    pub estimated_output_ms: f64,
}
/// Audio output seam. Device IO and recovery run outside the mixer callback.
pub trait DeviceBackend {
    /// Input sample rate required by submit.
    fn sample_rate(&self) -> u32;
    /// Smallest safe input watermark, including a quantum of refill headroom.
    fn minimum_queue_frames(&self) -> usize {
        0
    }
    /// Largest input watermark the device can hold. Checked before advancing the mixer.
    fn maximum_queue_frames(&self) -> usize {
        self.sample_rate() as usize
    }
    /// Checks device availability without consuming the platform's event queue.
    fn poll(&mut self) -> Result<DeviceEvent>;
    /// Appends stereo PCM to the output.
    fn submit(&mut self, interleaved: &[f32]) -> Result<()>;
    /// Frames queued at the input rate.
    fn queued_frames(&self) -> Result<usize>;
    /// Current estimated output latency.
    fn latency(&self) -> Result<Latency>;
    /// Attempts to reopen after loss. A failed attempt preserves the mixer graph.
    fn recover(&mut self) -> Result<DeviceEvent>;
}

/// Deterministic bounded offline sink for tests and agents. It never opens SDL.
pub struct OfflineBackend {
    rate: u32,
    buffer: Vec<f32>,
    max_frames: usize,
}
impl OfflineBackend {
    /// Reserves the complete capture capacity up front.
    pub fn new(rate: u32, max_frames: usize) -> Result<Self> {
        if !(8000..=192000).contains(&rate) || max_frames > rate as usize * 60 {
            return Err(AudioError::Invalid("offline capacity"));
        }
        Ok(Self {
            rate,
            buffer: Vec::with_capacity(max_frames * 2),
            max_frames,
        })
    }
    /// Completed interleaved capture.
    pub fn samples(&self) -> &[f32] {
        &self.buffer
    }
    /// Moves the completed interleaved capture out of the sink.
    pub fn into_samples(self) -> Vec<f32> {
        self.buffer
    }
}
impl DeviceBackend for OfflineBackend {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn poll(&mut self) -> Result<DeviceEvent> {
        Ok(DeviceEvent::Ready)
    }
    fn submit(&mut self, samples: &[f32]) -> Result<()> {
        if !samples.len().is_multiple_of(2) {
            return Err(AudioError::Invalid("stereo buffer length"));
        }
        if self.buffer.len() + samples.len() > self.max_frames * 2 {
            return Err(AudioError::Capacity("offline capture"));
        }
        self.buffer.extend_from_slice(samples);
        Ok(())
    }
    fn queued_frames(&self) -> Result<usize> {
        Ok(0)
    }
    fn latency(&self) -> Result<Latency> {
        Ok(Latency {
            queued_frames: 0,
            estimated_output_ms: 0.0,
        })
    }
    fn recover(&mut self) -> Result<DeviceEvent> {
        Ok(DeviceEvent::Ready)
    }
}

/// Copies exactly frames from the deterministic renderer to either sink.
pub fn render_to(
    engine: &mut AudioEngine,
    backend: &mut dyn DeviceBackend,
    frames: usize,
) -> Result<()> {
    if engine.sample_rate() != backend.sample_rate() {
        return Err(AudioError::Invalid("backend sample rate"));
    }
    let mut scratch = [0.0; QUANTUM * 2];
    let mut left = frames;
    while left > 0 {
        let n = left.min(QUANTUM);
        engine.render(&mut scratch[..n * 2])?;
        backend.submit(&scratch[..n * 2])?;
        left -= n;
    }
    Ok(())
}

/// Fills a real device to a bounded watermark. Call frequently on the audio worker.
/// No decoder, importer, graph construction or asset IO occurs here.
pub fn pump(
    engine: &mut AudioEngine,
    backend: &mut dyn DeviceBackend,
    target_frames: usize,
) -> Result<DeviceEvent> {
    if target_frames == 0 || target_frames > engine.sample_rate() as usize {
        return Err(AudioError::Invalid("device watermark"));
    }
    let event = backend.poll()?;
    if event == DeviceEvent::Lost {
        return Ok(event);
    }
    let target_frames = target_frames.max(backend.minimum_queue_frames());
    if target_frames > backend.maximum_queue_frames() {
        return Err(AudioError::Invalid(
            "device watermark exceeds native capacity",
        ));
    }
    let missing = target_frames.saturating_sub(backend.queued_frames()?);
    render_to(engine, backend, missing)?;
    Ok(event)
}

/// Copy the SDL thread-local error before the next SDL call.
fn sdl_error() -> AudioError {
    // SAFETY: SDL_GetError returns a live, NUL-terminated thread-local string.
    let message = unsafe { CStr::from_ptr(SDL_GetError()) }
        .to_string_lossy()
        .into_owned();
    AudioError::Backend(message)
}
/// Loads one C ABI symbol. Kept private so each use is type-checked against the audited signature.
pub(crate) fn symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T> {
    // SAFETY: all callers supply the exact pinned API signature; Library is retained for every pointer's lifetime.
    unsafe { library.get::<T>(name).map(|s| *s) }.map_err(|e| AudioError::Backend(e.to_string()))
}

/// SDL3 default output stream sharing the statically linked SDL instance with ara-platform.
/// It owns one SDL audio subsystem reference and never calls process-wide SDL_Quit.
pub struct SdlBackend {
    stream: *mut SDL_AudioStream,
    rate: u32,
    format: DeviceFormat,
    lost: bool,
}
impl SdlBackend {
    /// Opens stereo f32 output. SDL follows default-device migrations and performs rate conversion.
    pub fn open(sample_rate: u32) -> Result<Self> {
        if !(8000..=192000).contains(&sample_rate) {
            return Err(AudioError::Invalid("SDL sample rate"));
        }
        // SAFETY: static NUL-terminated hint strings. Environment overrides remain authoritative.
        // SDL's WASAPI backend negotiates IAudioClient3's nearest supported engine period.
        unsafe {
            SDL_SetHint(c"SDL_AUDIO_DEVICE_SAMPLE_FRAMES".as_ptr(), c"128".as_ptr());
        }
        // SAFETY: balanced by this backend's Drop; only the audio subsystem is initialized.
        if !unsafe { SDL_InitSubSystem(SDL_INIT_AUDIO) } {
            return Err(sdl_error());
        }
        let mut backend = Self {
            stream: ptr::null_mut(),
            rate: sample_rate,
            format: DeviceFormat {
                sample_rate,
                channels: 2,
                buffer_frames: 0,
            },
            lost: true,
        };
        backend.reopen()?;
        Ok(backend)
    }
    /// Current hardware format (not the f32 stereo stream's input format).
    pub fn format(&self) -> DeviceFormat {
        self.format
    }
    /// Active SDL driver name, for distinguishing WASAPI from fallback device paths.
    pub fn driver(&self) -> Result<String> {
        // SAFETY: SDL retains the driver name while the initialized subsystem is owned here.
        let name = unsafe { SDL_GetCurrentAudioDriver() };
        if name.is_null() {
            return Err(sdl_error());
        }
        // SAFETY: non-null SDL-owned NUL-terminated name.
        Ok(unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned())
    }
    /// Copies the selected output name for diagnostics, outside the callback.
    pub fn name(&self) -> Result<String> {
        if self.stream.is_null() {
            return Err(AudioError::Backend("device lost".into()));
        }
        // SAFETY: stream is live; SDL owns the NUL-terminated name returned for its device.
        let name = unsafe { SDL_GetAudioDeviceName(SDL_GetAudioStreamDevice(self.stream)) };
        if name.is_null() {
            return Err(sdl_error());
        }
        // SAFETY: non-null name returned by SDL_GetAudioDeviceName.
        Ok(unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned())
    }
    fn reopen(&mut self) -> Result<()> {
        if !self.stream.is_null() {
            // SAFETY: stream is uniquely owned here; SDL closes its associated logical device.
            unsafe { SDL_DestroyAudioStream(self.stream) };
            self.stream = ptr::null_mut();
        }
        self.lost = true;
        let spec = SDL_AudioSpec {
            format: SDL_AUDIO_F32,
            channels: 2,
            freq: self.rate as c_int,
        };
        // SAFETY: spec lives for this call; no callback/userdata is registered.
        self.stream = unsafe {
            SDL_OpenAudioDeviceStream(
                SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK,
                &spec,
                None,
                ptr::null_mut(),
            )
        };
        if self.stream.is_null() {
            return Err(sdl_error());
        }
        // SAFETY: newly opened stream is live and initially paused.
        if !unsafe { SDL_ResumeAudioStreamDevice(self.stream) } {
            return Err(sdl_error());
        }
        self.format = self.read_format()?;
        self.lost = false;
        Ok(())
    }
    fn read_format(&self) -> Result<DeviceFormat> {
        let mut spec = SDL_AudioSpec {
            format: SDL_AUDIO_F32,
            channels: 2,
            freq: 0,
        };
        let mut frames = 0;
        // SAFETY: stream is live and both output pointers reference writable stack values.
        if !unsafe {
            SDL_GetAudioDeviceFormat(
                SDL_GetAudioStreamDevice(self.stream),
                &mut spec,
                &mut frames,
            )
        } {
            return Err(sdl_error());
        }
        if spec.freq <= 0 || spec.channels <= 0 || frames < 0 {
            return Err(AudioError::Backend("invalid SDL device format".into()));
        }
        Ok(DeviceFormat {
            sample_rate: spec.freq as u32,
            channels: spec.channels as u16,
            buffer_frames: frames as u32,
        })
    }
}
impl DeviceBackend for SdlBackend {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn minimum_queue_frames(&self) -> usize {
        (u64::from(self.format.buffer_frames) * u64::from(self.rate))
            .div_ceil(u64::from(self.format.sample_rate)) as usize
            + QUANTUM
    }
    fn poll(&mut self) -> Result<DeviceEvent> {
        if self.lost {
            return Ok(DeviceEvent::Lost);
        }
        match self.read_format() {
            Ok(format) => {
                let changed = format != self.format;
                self.format = format;
                Ok(if changed {
                    DeviceEvent::Changed
                } else {
                    DeviceEvent::Ready
                })
            }
            Err(_) => {
                self.lost = true;
                Ok(DeviceEvent::Lost)
            }
        }
    }
    fn submit(&mut self, samples: &[f32]) -> Result<()> {
        if self.lost {
            return Err(AudioError::Backend(
                "device lost; recover on audio worker".into(),
            ));
        }
        if !samples.len().is_multiple_of(2) {
            return Err(AudioError::Invalid("stereo buffer length"));
        }
        let bytes = c_int::try_from(std::mem::size_of_val(samples))
            .map_err(|_| AudioError::Capacity("SDL submission"))?;
        // SAFETY: SDL copies bytes before returning, and samples contains exactly bytes initialized bytes.
        if !unsafe { SDL_PutAudioStreamData(self.stream, samples.as_ptr().cast(), bytes) } {
            self.lost = true;
            return Err(sdl_error());
        }
        Ok(())
    }
    fn queued_frames(&self) -> Result<usize> {
        if self.lost {
            return Err(AudioError::Backend("device lost".into()));
        }
        // SAFETY: live, uniquely owned stream; SDL synchronizes its internal hardware consumer.
        let bytes = unsafe { SDL_GetAudioStreamQueued(self.stream) };
        if bytes < 0 {
            return Err(sdl_error());
        }
        Ok(bytes as usize / 8)
    }
    fn latency(&self) -> Result<Latency> {
        let queued = self.queued_frames()?;
        Ok(Latency {
            queued_frames: queued,
            estimated_output_ms: 1000.0
                * (queued as f64 / self.rate as f64
                    + self.format.buffer_frames as f64 / self.format.sample_rate as f64),
        })
    }
    fn recover(&mut self) -> Result<DeviceEvent> {
        self.reopen()?;
        Ok(DeviceEvent::Recovered)
    }
}
impl Drop for SdlBackend {
    fn drop(&mut self) {
        // SAFETY: destroy only our stream, then release exactly our audio subsystem reference without touching other SDL owners.
        unsafe {
            if !self.stream.is_null() {
                SDL_DestroyAudioStream(self.stream);
            }
            SDL_QuitSubSystem(SDL_INIT_AUDIO);
        }
    }
}
