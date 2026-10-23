//! Headless S7 export conformance template using production pack and bridge APIs.
//! Full engine templates replace this fixture through the same pinned artifact seam.
#![warn(missing_docs)]
pub mod abi;
#[cfg(target_os = "android")]
pub mod android;
mod columns;
#[cfg(feature = "dev-bridge")]
pub mod observability;
pub mod runtime;
mod script;

/// Platform log sink (stdout or Android logcat).
pub mod log {
    /// Emits a C#/Rust boundary log message.
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
