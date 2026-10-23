//! Crash capture stays local; symbolization and upload run outside the failing process.
#![warn(missing_docs)]

mod capture;
mod report;
#[cfg(feature = "symbols")]
pub mod symbols;
#[cfg(feature = "explain")]
pub mod why;

pub use capture::{Capture, monitor_from_args};
pub use report::{CrashKind, CrashReport, CrashSettings, UploadConsent, upload, write_report};

/// Stable diagnostic emitted by symbolized native, Rust and managed crash reports.
pub const CRASH: ara_diag::Code = ara_diag::Code(2701);

/// Deliberately raises a native exception/signal in an explicitly launched test child.
/// This never runs as part of normal capture or application startup.
#[inline(never)]
pub fn native_crash_probe() -> ! {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn RaiseException(code: u32, flags: u32, count: u32, arguments: *const usize);
        }
        // SAFETY: zero arguments permits a null argument pointer. The deliberately
        // unhandled exception terminates this test process after its handler runs.
        unsafe { RaiseException(0xC0000005, 1, 0, std::ptr::null()) };
    }
    std::process::abort()
}

pub(crate) fn canonical<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    Ok(ara_json::to_canonical_string(&ara_json::sort_keys(
        &serde_json::to_value(value)?,
    )))
}
