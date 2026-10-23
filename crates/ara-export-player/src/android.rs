//! Android entry. `android.app.NativeActivity` loads `libara_export_player.so` and calls
//! [`ANativeActivity_onCreate`]; the APK has no dex (`android:hasCode="false"`).
//!
//! The pack is an *uncompressed* APK asset, so `AAsset_getBuffer` maps it straight
//! from the APK. The C# NativeAOT image is `libgame.so` in the same `lib/<abi>/`.

use std::ffi::{CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::time::Instant;

use crate::runtime::{RunConfig, ScriptLane, run};

/// `ANativeActivity` from `<android/native_activity.h>`.
#[repr(C)]
pub struct ANativeActivity {
    callbacks: *mut c_void,
    vm: *mut c_void,
    env: *mut c_void,
    clazz: *mut c_void,
    internal_data_path: *const c_char,
    external_data_path: *const c_char,
    sdk_version: i32,
    instance: *mut c_void,
    asset_manager: *mut c_void,
    obb_path: *const c_char,
}

const AASSET_MODE_BUFFER: c_int = 3;

#[link(name = "android")]
unsafe extern "C" {
    fn AAssetManager_open(mgr: *mut c_void, filename: *const c_char, mode: c_int) -> *mut c_void;
    fn AAsset_getBuffer(asset: *mut c_void) -> *const c_void;
    fn AAsset_getLength64(asset: *mut c_void) -> i64;
    fn AAsset_isAllocated(asset: *mut c_void) -> c_int;
    fn AAsset_close(asset: *mut c_void);
    fn ANativeActivity_finish(activity: *mut ANativeActivity);
}

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// Writes one logcat line under tag `ara`.
pub fn log(level: u32, msg: &str) {
    let prio = match level {
        0 => 3, // DEBUG
        1 => 4, // INFO
        _ => 6, // ERROR
    };
    if let Ok(text) = CString::new(msg) {
        // SAFETY: both strings are NUL-terminated and live for the call.
        unsafe { __android_log_write(prio, c"ara".as_ptr(), text.as_ptr()) };
    }
}

/// Called by NativeActivity on the main thread. Runs the game on a worker thread, then finishes.
///
/// # Safety
/// `activity` must be the live `ANativeActivity` passed by the framework.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ANativeActivity_onCreate(
    activity: *mut ANativeActivity,
    _saved: *mut c_void,
    _len: usize,
) {
    let t0 = Instant::now();
    // This headless fixture runs synchronously, so onDestroy cannot invalidate activity.
    if let Err(e) = run_activity(activity, t0) {
        log(2, &format!("[rt] fatal: {e}"));
    }
    // SAFETY: the activity remains live throughout its onCreate callback.
    unsafe { ANativeActivity_finish(activity) };
}

fn run_activity(activity: *mut ANativeActivity, t0: Instant) -> Result<(), String> {
    #[cfg(feature = "dev-bridge")]
    let bridge = {
        // SAFETY: activity is live during onCreate and Android owns this NUL-terminated path.
        let private = unsafe { std::ffi::CStr::from_ptr((*activity).internal_data_path) };
        crate::observability::start_private(std::path::Path::new(
            private.to_str().map_err(|e| e.to_string())?,
        ))?
    };
    // SAFETY: the framework keeps the activity (and its asset manager) alive until onDestroy.
    let manager = unsafe { (*activity).asset_manager };
    // SAFETY: valid manager and NUL-terminated name.
    let asset =
        unsafe { AAssetManager_open(manager, c"game.arapack".as_ptr(), AASSET_MODE_BUFFER) };
    if asset.is_null() {
        return Err("asset game.arapack not found".into());
    }
    // SAFETY: `asset` is open; the buffer stays valid until AAsset_close below.
    let (ptr, len, copied) = unsafe {
        (
            AAsset_getBuffer(asset),
            AAsset_getLength64(asset),
            AAsset_isAllocated(asset),
        )
    };
    if ptr.is_null() || len < 0 {
        // SAFETY: closing the asset we opened.
        unsafe { AAsset_close(asset) };
        return Err("could not map game.arapack".into());
    }
    // SAFETY: AAsset_getBuffer returned `len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
    log(
        1,
        &format!(
            "[rt] game.arapack {len} bytes, {}",
            if copied != 0 {
                "decompressed copy"
            } else {
                "mapped from APK"
            }
        ),
    );
    let cfg = RunConfig {
        crash_probe: None,
        pack_key: std::env::var("ARA_PACK_KEY")
            .ok()
            .map(|s| ara_pack::EncryptionKey::from_hex(&s).map_err(|e| e.to_string()))
            .transpose()?,
        lane: ScriptLane::Aot {
            lib: PathBuf::from("libgame.so"),
        },
        ticks: None,
        bench_iters: 0,
        verify: true,
        mod_path: None,
        spawn_unix_ns: None,
    };
    let result = run(&cfg, bytes, t0);
    // SAFETY: `bytes` is not used after this point.
    unsafe { AAsset_close(asset) };
    let report = result?;
    #[cfg(feature = "dev-bridge")]
    if let Some(bridge) = &bridge {
        crate::observability::report(bridge, &report);
    }
    let json = serde_json::to_string(&report).map_err(|e| e.to_string())?;
    log(1, &format!("ARA_RESULT {json}"));
    Ok(())
}
