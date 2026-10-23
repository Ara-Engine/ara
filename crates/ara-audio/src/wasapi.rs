//! Low-latency Windows shared output. RAW GameEffects bypasses endpoint processing;
//! IAudioClient3 negotiates the driver period, without exclusive device ownership.
use crate::{
    AudioError, Result,
    device::{DeviceBackend, DeviceEvent, Latency},
};
use std::{marker::PhantomData, rc::Rc};
use windows::{
    Win32::{
        Foundation::HANDLE,
        Media::Audio::*,
        System::{Com::*, Threading::*},
    },
    core::GUID,
};

/// Shared-stream settings, chosen on the presentation worker before playback.
#[derive(Debug, Clone, Copy)]
pub struct WasapiOptions {
    /// Requested period, rounded up to a driver-supported multiple and clamped to its limits.
    pub period_frames: u32,
    /// Bypass OS effects. Unsupported RAW mode is diagnosed, never silently replaced.
    pub raw: bool,
}
impl Default for WasapiOptions {
    fn default() -> Self {
        Self {
            period_frames: 128,
            raw: true,
        }
    }
}
fn error(e: windows::core::Error) -> AudioError {
    AudioError::Backend(format!("WASAPI: {e}"))
}

// Last backend field: interfaces must be released before COM teardown.
struct Apartment {
    task: HANDLE,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: !Send guard is dropped on the thread that initialized/registered it.
        unsafe {
            if !self.task.is_invalid() {
                let _ = AvRevertMmThreadCharacteristics(self.task);
            }
            CoUninitialize();
        }
    }
}
struct Format(*mut WAVEFORMATEX);
impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: GetMixFormat allocated this pointer; it is freed once after its final use.
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
struct Device {
    render: IAudioRenderClient,
    client: IAudioClient3,
    id: String,
    period: u32,
    capacity: u32,
    reported_latency: i64,
}
fn device_id(device: &IMMDevice) -> Result<String> {
    // SAFETY: live interface on this apartment; GetId returns a COM-allocated string.
    unsafe {
        let id = device.GetId().map_err(error)?;
        let result = id
            .to_string()
            .map_err(|e| AudioError::Backend(e.to_string()));
        CoTaskMemFree(Some(id.0.cast()));
        result
    }
}
impl Device {
    fn open(enumerator: &IMMDeviceEnumerator, rate: u32, options: WasapiOptions) -> Result<Self> {
        // SAFETY: this worker owns the apartment and all interfaces/buffers below.
        unsafe {
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(error)?;
            let id = device_id(&device)?;
            let client: IAudioClient3 = device.Activate(CLSCTX_ALL, None).map_err(error)?;
            client
                .SetClientProperties(&AudioClientProperties {
                    cbSize: size_of::<AudioClientProperties>() as u32,
                    bIsOffload: false.into(),
                    eCategory: AudioCategory_GameEffects,
                    Options: if options.raw {
                        AUDCLNT_STREAMOPTIONS_RAW
                    } else {
                        AUDCLNT_STREAMOPTIONS_NONE
                    },
                })
                .map_err(error)?;
            let format = Format(client.GetMixFormat().map_err(error)?);
            let wave = &*format.0;
            let float = wave.wFormatTag == 3
                || (wave.wFormatTag == 0xfffe && wave.cbSize >= 22 && {
                    let subformat = (*(format.0.cast::<WAVEFORMATEXTENSIBLE>())).SubFormat;
                    subformat == GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71)
                });
            if wave.nSamplesPerSec != rate
                || wave.nChannels != 2
                || wave.wBitsPerSample != 32
                || wave.nBlockAlign != 8
                || !float
            {
                return Err(AudioError::Backend("WASAPI fast output requires a matching stereo f32 mix format; use SdlBackend for format conversion".into()));
            }
            let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
            client
                .GetSharedModeEnginePeriod(
                    format.0,
                    &mut default,
                    &mut fundamental,
                    &mut min,
                    &mut max,
                )
                .map_err(error)?;
            if fundamental == 0 || min == 0 || max < min {
                return Err(AudioError::Invalid("WASAPI engine periods"));
            }
            let period = options
                .period_frames
                .div_ceil(fundamental)
                .saturating_mul(fundamental)
                .clamp(min, max);
            client
                .InitializeSharedAudioStream(0, period, format.0, None)
                .map_err(error)?;
            let capacity = client.GetBufferSize().map_err(error)?;
            if capacity < period {
                return Err(AudioError::Invalid("WASAPI buffer capacity"));
            }
            let reported_latency = client.GetStreamLatency().map_err(error)?;
            let render = client.GetService().map_err(error)?;
            client.Start().map_err(error)?;
            Ok(Self {
                render,
                client,
                id,
                period,
                capacity,
                reported_latency,
            })
        }
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: stop the uniquely owned client before releasing its render service.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// Windows 10+ shared sink. Construct, pump, recover and drop on one audio worker.
/// Unsupported stereo f32 mix formats return an error; SDL remains the conversion backend.
pub struct WasapiBackend {
    device: Option<Device>,
    enumerator: IMMDeviceEnumerator,
    rate: u32,
    options: WasapiOptions,
    polls: u8,
    _apartment: Apartment,
}
impl WasapiBackend {
    /// Opens the default console output and registers this worker with MMCSS Pro Audio.
    /// This changes no process-wide timer settings and acquires no exclusive device lock.
    pub fn open(rate: u32, options: WasapiOptions) -> Result<Self> {
        if !(8000..=192000).contains(&rate) || options.period_frames == 0 {
            return Err(AudioError::Invalid("WASAPI rate/period"));
        }
        // SAFETY: balanced !Send guard outlives every native interface.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(error)?;
            let mut index = 0;
            let task = AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut index)
                .unwrap_or_default();
            let apartment = Apartment {
                task,
                _thread: PhantomData,
            };
            let enumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(error)?;
            let device = Device::open(&enumerator, rate, options)?;
            Ok(Self {
                device: Some(device),
                enumerator,
                rate,
                options,
                polls: 0,
                _apartment: apartment,
            })
        }
    }
    fn device(&self) -> Result<&Device> {
        self.device
            .as_ref()
            .ok_or_else(|| AudioError::Backend("WASAPI device lost; recover on worker".into()))
    }
    /// Negotiated shared engine period in device/engine frames.
    pub fn period_frames(&self) -> Result<u32> {
        Ok(self.device()?.period)
    }
    /// Total native capacity. Submission fills one period, not this whole buffer.
    pub fn capacity_frames(&self) -> Result<u32> {
        Ok(self.device()?.capacity)
    }
    /// Stable endpoint ID used to detect default output changes.
    pub fn endpoint_id(&self) -> Result<&str> {
        Ok(&self.device()?.id)
    }
    /// Driver stream-latency report in milliseconds; some endpoints return zero.
    pub fn reported_latency_ms(&self) -> Result<f64> {
        Ok(self.device()?.reported_latency as f64 / 10000.0)
    }
}
impl DeviceBackend for WasapiBackend {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn minimum_queue_frames(&self) -> usize {
        self.device.as_ref().map_or(0, |d| d.period as usize)
    }
    fn maximum_queue_frames(&self) -> usize {
        self.device.as_ref().map_or(0, |d| d.capacity as usize)
    }
    fn poll(&mut self) -> Result<DeviceEvent> {
        if self.device.is_none() {
            return Ok(DeviceEvent::Lost);
        }
        self.polls = self.polls.wrapping_add(1);
        let ready = self.queued_frames().is_ok()
            && (self.polls != 0 || {
                // SAFETY: live enumerator on this apartment. ID copies are device IO, outside render.
                unsafe { self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                    .map_err(error)
                    .and_then(|d| device_id(&d))
                    .is_ok_and(|id| self.device.as_ref().is_some_and(|d| id == d.id))
            });
        if ready {
            Ok(DeviceEvent::Ready)
        } else {
            self.device = None;
            Ok(DeviceEvent::Lost)
        }
    }
    fn submit(&mut self, pcm: &[f32]) -> Result<()> {
        if !pcm.len().is_multiple_of(2) {
            return Err(AudioError::Invalid("stereo buffer length"));
        }
        if pcm.is_empty() {
            return Ok(());
        }
        let device = self.device()?;
        let frames =
            u32::try_from(pcm.len() / 2).map_err(|_| AudioError::Capacity("WASAPI submission"))?;
        if frames > device.capacity.saturating_sub(self.queued_frames()? as u32) {
            return Err(AudioError::Capacity("WASAPI submission"));
        }
        // SAFETY: returned buffer has frames * 2 f32 slots in the validated mix format.
        unsafe {
            let buffer = device.render.GetBuffer(frames).map_err(error)?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr(), buffer.cast(), pcm.len());
            device.render.ReleaseBuffer(frames, 0).map_err(error)
        }
    }
    fn queued_frames(&self) -> Result<usize> {
        // SAFETY: live client on its owning apartment; reads only its padding counter.
        unsafe { self.device()?.client.GetCurrentPadding() }
            .map(|n| n as usize)
            .map_err(error)
    }
    fn latency(&self) -> Result<Latency> {
        let queued_frames = self.queued_frames()?;
        Ok(Latency {
            queued_frames,
            estimated_output_ms: 1000.0 * (queued_frames as f64 + self.device()?.period as f64)
                / self.rate as f64
                + self.reported_latency_ms()?,
        })
    }
    fn recover(&mut self) -> Result<DeviceEvent> {
        self.device = None;
        self.device = Some(Device::open(&self.enumerator, self.rate, self.options)?);
        Ok(DeviceEvent::Recovered)
    }
}
