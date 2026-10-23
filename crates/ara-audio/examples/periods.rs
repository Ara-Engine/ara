//! Explicit Windows capability probe; initializes a paused client and emits no audio.
#[cfg(windows)]
struct Apartment;
#[cfg(windows)]
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: declared before all interfaces, and dropped on this probe's COM thread.
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}
#[cfg(windows)]
struct Format(*mut windows::Win32::Media::Audio::WAVEFORMATEX);
#[cfg(windows)]
impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: owns exactly one GetMixFormat allocation, including on early errors.
        unsafe {
            windows::Win32::System::Com::CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::Win32::{Media::Audio::*, System::Com::*};
    // SAFETY: standalone diagnostic thread owns its COM apartment and all interface lifetimes.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let _apartment = Apartment;
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient3 = device.Activate(CLSCTX_ALL, None)?;
        let format = Format(client.GetMixFormat()?);
        let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
        let result = client.GetSharedModeEnginePeriod(
            format.0,
            &mut default,
            &mut fundamental,
            &mut min,
            &mut max,
        );
        let (rate, channels) = ((*format.0).nSamplesPerSec, (*format.0).nChannels);
        println!(
            "rate={} channels={} periods={result:?}: default={default} fundamental={fundamental} min={min} max={max}",
            rate, channels
        );
        let initialized = client.InitializeSharedAudioStream(0, min, format.0, None);
        println!("initialize minimum period: {initialized:?}");
        if initialized.is_ok() {
            println!(
                "buffer={} latency100ns={}",
                client.GetBufferSize()?,
                client.GetStreamLatency()?
            );
        }
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows endpoint capability probe");
}
