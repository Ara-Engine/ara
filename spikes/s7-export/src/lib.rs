//! S7 runtime template core: content pack, R10-shaped C# ABI, and the run loop.
//!
//! Desktop entry: `src/bin/ara-player.rs`. Android entry: [`android`] (`libara_runtime.so`
//! loaded by `android.app.NativeActivity`, no Java code).

pub mod abi;
pub mod pack;
pub mod runtime;

#[cfg(target_os = "android")]
pub mod android;

/// Log sink: stdout on desktop, logcat (tag `ara`) on Android.
pub mod log {
    /// Writes one line. `level`: 0 debug, 1 info, 2 error. `tag`: `rt` (Rust) or `cs` (C#).
    pub fn write(level: u32, tag: &str, msg: &str) {
        #[cfg(target_os = "android")]
        crate::android::log(level, &format!("[{tag}] {msg}"));
        #[cfg(not(target_os = "android"))]
        {
            let _ = level;
            println!("[{tag}] {msg}");
        }
    }
}
