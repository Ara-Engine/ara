//! Thin, owned Steam Audio 4.8.1 C ABI binding. Native handles never escape this module.
//! Default measured HRTF or a custom SOFA file; no synthetic HRTF approximation.

use crate::{
    AudioError, Frame, QUANTUM, Result,
    device::symbol,
    spatial::{AcousticPath, Spatializer},
};
use libloading::Library;
use std::{
    ffi::{CString, c_char, c_void},
    path::Path,
    ptr,
    sync::Arc,
};

type Handle = *mut c_void;
#[repr(C)]
struct ContextSettings {
    version: u32,
    log: Option<unsafe extern "C" fn(i32, *const c_char)>,
    allocate: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
    free: Option<unsafe extern "C" fn(*mut c_void)>,
    simd: i32,
    flags: i32,
}
#[repr(C)]
struct AudioSettings {
    rate: i32,
    frame_size: i32,
}
#[repr(C)]
struct HrtfSettings {
    kind: i32,
    sofa_file: *const c_char,
    sofa_data: *const u8,
    sofa_size: i32,
    volume: f32,
    norm: i32,
}
#[repr(C)]
struct BinauralSettings {
    hrtf: Handle,
}
#[repr(C)]
struct BinauralParams {
    direction: [f32; 3],
    interpolation: i32,
    blend: f32,
    hrtf: Handle,
    delays: *mut f32,
}
#[repr(C)]
struct DirectSettings {
    channels: i32,
}
#[repr(C)]
struct DirectParams {
    flags: i32,
    transmission_type: i32,
    distance: f32,
    air: [f32; 3],
    directivity: f32,
    occlusion: f32,
    transmission: [f32; 3],
}
#[repr(C)]
struct Buffer {
    channels: i32,
    samples: i32,
    data: *mut *mut f32,
}

type ContextCreate = unsafe extern "C" fn(*mut ContextSettings, *mut Handle) -> i32;
type Release = unsafe extern "C" fn(*mut Handle);
type HrtfCreate =
    unsafe extern "C" fn(Handle, *mut AudioSettings, *mut HrtfSettings, *mut Handle) -> i32;
type BinauralCreate =
    unsafe extern "C" fn(Handle, *mut AudioSettings, *mut BinauralSettings, *mut Handle) -> i32;
type BinauralApply =
    unsafe extern "C" fn(Handle, *mut BinauralParams, *mut Buffer, *mut Buffer) -> i32;
type BinauralTail = unsafe extern "C" fn(Handle, *mut Buffer) -> i32;
type DirectCreate =
    unsafe extern "C" fn(Handle, *mut AudioSettings, *mut DirectSettings, *mut Handle) -> i32;
type DirectApply = unsafe extern "C" fn(Handle, *mut DirectParams, *mut Buffer, *mut Buffer) -> i32;

struct Api {
    context_create: ContextCreate,
    context_release: Release,
    hrtf_create: HrtfCreate,
    hrtf_release: Release,
    binaural_create: BinauralCreate,
    binaural_release: Release,
    binaural_apply: BinauralApply,
    binaural_tail: BinauralTail,
    direct_create: DirectCreate,
    direct_release: Release,
    direct_apply: DirectApply,
    _library: Library,
}
impl Api {
    fn load(path: &Path) -> Result<Self> {
        // SAFETY: path is a trusted, pinned Steam Audio runtime supplied by the engine/loader.
        let lib = unsafe { Library::new(path) }.map_err(|e| AudioError::Backend(e.to_string()))?;
        Ok(Self {
            context_create: symbol(&lib, b"iplContextCreate\0")?,
            context_release: symbol(&lib, b"iplContextRelease\0")?,
            hrtf_create: symbol(&lib, b"iplHRTFCreate\0")?,
            hrtf_release: symbol(&lib, b"iplHRTFRelease\0")?,
            binaural_create: symbol(&lib, b"iplBinauralEffectCreate\0")?,
            binaural_release: symbol(&lib, b"iplBinauralEffectRelease\0")?,
            binaural_apply: symbol(&lib, b"iplBinauralEffectApply\0")?,
            binaural_tail: symbol(&lib, b"iplBinauralEffectGetTail\0")?,
            direct_create: symbol(&lib, b"iplDirectEffectCreate\0")?,
            direct_release: symbol(&lib, b"iplDirectEffectRelease\0")?,
            direct_apply: symbol(&lib, b"iplDirectEffectApply\0")?,
            _library: lib,
        })
    }
}
struct Context {
    api: Api,
    context: Handle,
    hrtf: Handle,
    rate: u32,
}
// SAFETY: context/HRTF are immutable after construction; Steam effects are independently owned per voice.
// Steam's C API permits sharing the retained context and HRTF between effects on different threads.
unsafe impl Send for Context {}
// SAFETY: no mutable context/HRTF access is exposed, and effect instances are never shared.
unsafe impl Sync for Context {}
impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: last Arc owner releases HRTF before context, while the library is still loaded.
        unsafe {
            if !self.hrtf.is_null() {
                (self.api.hrtf_release)(&mut self.hrtf);
            }
            if !self.context.is_null() {
                (self.api.context_release)(&mut self.context);
            }
        }
    }
}

/// Shared native HRTF resources. Construct on an asset worker, then prepare one effect per voice.
#[derive(Clone)]
pub struct SteamAudio {
    context: Arc<Context>,
}
impl SteamAudio {
    /// Loads Steam Audio 4.8.1 and its built-in measured HRTF, or a custom SOFA dataset.
    /// Deployment downloads remain explicit; opening an offline engine needs no native library.
    pub fn load(path: impl AsRef<Path>, sample_rate: u32, sofa: Option<&Path>) -> Result<Self> {
        if !(8000..=192000).contains(&sample_rate) {
            return Err(AudioError::Invalid("HRTF sample rate"));
        }
        let api = Api::load(path.as_ref())?;
        let mut context = Context {
            api,
            context: ptr::null_mut(),
            hrtf: ptr::null_mut(),
            rate: sample_rate,
        };
        let mut settings = ContextSettings {
            version: 0x00040801,
            log: None,
            allocate: Some(crate::steam_alloc::allocate),
            free: Some(crate::steam_alloc::free),
            simd: 3,
            flags: 0,
        };
        check(
            // SAFETY: C-compatible settings/output are valid; allocator callbacks honor the pinned ABI.
            unsafe { (context.api.context_create)(&mut settings, &mut context.context) },
            "context",
        )?;
        let sofa = sofa
            .map(|p| {
                CString::new(p.to_string_lossy().as_bytes())
                    .map_err(|_| AudioError::Invalid("SOFA path contains NUL"))
            })
            .transpose()?;
        let mut hrtf = HrtfSettings {
            kind: i32::from(sofa.is_some()),
            sofa_file: sofa.as_ref().map_or(ptr::null(), |p| p.as_ptr()),
            sofa_data: ptr::null(),
            sofa_size: 0,
            volume: 1.0,
            norm: 0,
        };
        let mut audio = AudioSettings {
            rate: sample_rate as i32,
            frame_size: QUANTUM as i32,
        };
        check(
            // SAFETY: creation copies/loads SOFA synchronously; all pointers remain live for this call.
            unsafe {
                (context.api.hrtf_create)(context.context, &mut audio, &mut hrtf, &mut context.hrtf)
            },
            "HRTF",
        )?;
        Ok(Self {
            context: Arc::new(context),
        })
    }
    /// Constructs binaural and three-band direct-path effects, allocating outside rendering.
    pub fn spatializer(&self) -> Result<SteamSpatializer> {
        let mut effect = SteamSpatializer {
            context: Arc::clone(&self.context),
            binaural: ptr::null_mut(),
            direct: ptr::null_mut(),
            input: [0.0; QUANTUM],
            filtered: [0.0; QUANTUM],
            left: [0.0; QUANTUM],
            right: [0.0; QUANTUM],
        };
        let mut audio = AudioSettings {
            rate: self.context.rate as i32,
            frame_size: QUANTUM as i32,
        };
        let mut settings = BinauralSettings {
            hrtf: self.context.hrtf,
        };
        check(
            // SAFETY: valid context/HRTF retained by effect; output is uniquely borrowed.
            unsafe {
                (self.context.api.binaural_create)(
                    self.context.context,
                    &mut audio,
                    &mut settings,
                    &mut effect.binaural,
                )
            },
            "binaural effect",
        )?;
        let mut settings = DirectSettings { channels: 1 };
        check(
            // SAFETY: valid context and C-layout settings; ownership transfers to effect on success.
            unsafe {
                (self.context.api.direct_create)(
                    self.context.context,
                    &mut audio,
                    &mut settings,
                    &mut effect.direct,
                )
            },
            "direct effect",
        )?;
        Ok(effect)
    }
}
fn check(code: i32, operation: &str) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(AudioError::Backend(format!(
            "Steam Audio {operation} failed ({code})"
        )))
    }
}

/// Per-voice Steam Audio state, including frequency-dependent occlusion/transmission and HRTF convolution.
pub struct SteamSpatializer {
    context: Arc<Context>,
    binaural: Handle,
    direct: Handle,
    input: [f32; QUANTUM],
    filtered: [f32; QUANTUM],
    left: [f32; QUANTUM],
    right: [f32; QUANTUM],
}
// SAFETY: each effect has one owner and can move between threads while idle; process requires &mut self.
unsafe impl Send for SteamSpatializer {}
impl Spatializer for SteamSpatializer {
    fn process(
        &mut self,
        mono: &[f32],
        out: &mut [Frame],
        direction: [f32; 3],
        path: AcousticPath,
    ) {
        if mono.len() != QUANTUM || out.len() != QUANTUM {
            out.fill(Frame::ZERO);
            return;
        }
        self.input.copy_from_slice(mono);
        let mut input_channels = [self.input.as_mut_ptr()];
        let mut filtered_channels = [self.filtered.as_mut_ptr()];
        let mut output_channels = [self.left.as_mut_ptr(), self.right.as_mut_ptr()];
        let mut input = Buffer {
            channels: 1,
            samples: QUANTUM as i32,
            data: input_channels.as_mut_ptr(),
        };
        let mut filtered = Buffer {
            channels: 1,
            samples: QUANTUM as i32,
            data: filtered_channels.as_mut_ptr(),
        };
        let mut output = Buffer {
            channels: 2,
            samples: QUANTUM as i32,
            data: output_channels.as_mut_ptr(),
        };
        let mut direct = DirectParams {
            flags: 8 | 16,
            transmission_type: 1,
            distance: 1.0,
            air: [1.0; 3],
            directivity: 1.0,
            occlusion: path.visibility,
            transmission: path.transmission,
        };
        let length = direction
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt()
            .max(0.00001);
        let mut binaural = BinauralParams {
            direction: direction.map(|v| v / length),
            interpolation: 1,
            blend: 1.0,
            hrtf: self.context.hrtf,
            delays: ptr::null_mut(),
        };
        // SAFETY: distinct, initialized planar arrays are exactly the configured quantum; native effects are uniquely owned.
        unsafe {
            (self.context.api.direct_apply)(self.direct, &mut direct, &mut input, &mut filtered);
            (self.context.api.binaural_apply)(
                self.binaural,
                &mut binaural,
                &mut filtered,
                &mut output,
            );
        }
        for (i, frame) in out.iter_mut().enumerate() {
            *frame = Frame::new(self.left[i], self.right[i]);
        }
    }
    fn drain(&mut self, out: &mut [Frame]) -> bool {
        if out.len() != QUANTUM {
            out.fill(Frame::ZERO);
            return true;
        }
        let mut channels = [self.left.as_mut_ptr(), self.right.as_mut_ptr()];
        let mut buffer = Buffer {
            channels: 2,
            samples: QUANTUM as i32,
            data: channels.as_mut_ptr(),
        };
        // SAFETY: planar arrays match the effect's configured quantum and the native effect has one owner.
        let state = unsafe { (self.context.api.binaural_tail)(self.binaural, &mut buffer) };
        for (i, frame) in out.iter_mut().enumerate() {
            *frame = Frame::new(self.left[i], self.right[i]);
        }
        state == 1
    }
}
impl Drop for SteamSpatializer {
    fn drop(&mut self) {
        // SAFETY: unique effect handles are released before the retained context and library can be destroyed.
        unsafe {
            if !self.direct.is_null() {
                (self.context.api.direct_release)(&mut self.direct);
            }
            if !self.binaural.is_null() {
                (self.context.api.binaural_release)(&mut self.binaural);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c_abi_layout_matches_steam_audio_481_on_64_bit() {
        if size_of::<usize>() == 8 {
            assert_eq!(size_of::<ContextSettings>(), 40);
            assert_eq!(size_of::<HrtfSettings>(), 40);
            assert_eq!(size_of::<BinauralParams>(), 40);
            assert_eq!(size_of::<DirectParams>(), 44);
            assert_eq!(size_of::<Buffer>(), 16);
        }
    }
}
