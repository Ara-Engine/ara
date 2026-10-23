//! Crash isolation, player side: every fatal path writes exactly one machine-readable
//! `ARA_PLAYER_CRASH {json}` line to stderr, then the process exits at once with its
//! natural code (101 for a panic, the NTSTATUS for an exception). The owner pipes
//! stderr, classifies the exit and runs the heartbeat watchdog for hangs.
use ara_play::{CRASH_PREFIX, CrashReport};
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

/// Last completed tick, for crash reports (diagnostic only, never simulation state).
pub static LAST_TICK: AtomicU64 = AtomicU64::new(0);

fn emit(report: &CrashReport) {
    if let Ok(json) = serde_json::to_string(report) {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{CRASH_PREFIX}{json}");
        let _ = err.flush();
    }
}

/// Installs the panic hook and (on Windows) the unhandled-exception filter, and
/// suppresses the Windows Error Reporting dialog.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        emit(&CrashReport {
            kind: "panic".into(),
            message,
            location,
            tick: LAST_TICK.load(Ordering::Relaxed),
        });
    }));
    #[cfg(windows)]
    windows::install();
}

#[cfg(windows)]
mod windows {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
        SetUnhandledExceptionFilter,
    };

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the OS passes valid exception pointers to the top-level filter.
        let (code, address) = unsafe {
            let record = (*info).ExceptionRecord;
            if record.is_null() {
                (0, 0)
            } else {
                (
                    (*record).ExceptionCode as u32,
                    (*record).ExceptionAddress as usize,
                )
            }
        };
        let name = match code {
            0xC000_0005 => "STATUS_ACCESS_VIOLATION",
            0xC000_00FD => "STATUS_STACK_OVERFLOW",
            0xC000_001D => "STATUS_ILLEGAL_INSTRUCTION",
            0xC000_0094 => "STATUS_INTEGER_DIVIDE_BY_ZERO",
            _ => "exception",
        };
        emit(&CrashReport {
            kind: "exception".into(),
            message: format!("{name} (0x{code:08X})"),
            location: format!("0x{address:X}"),
            tick: LAST_TICK.load(Ordering::Relaxed),
        });
        // EXCEPTION_EXECUTE_HANDLER: terminate with the exception code.
        1
    }

    pub fn install() {
        // SAFETY: process-wide error-mode and filter registration with a static fn.
        unsafe {
            SetErrorMode(SEM_NOGPFAULTERRORBOX | SEM_FAILCRITICALERRORS);
            SetUnhandledExceptionFilter(Some(filter));
        }
    }
}

/// `play.debug.crash`: deliberately fail in the requested way (fault-isolation tests).
pub fn trigger(kind: &str) -> ! {
    match kind {
        "access" => {
            // Raises a genuine STATUS_ACCESS_VIOLATION through SEH without undefined
            // behaviour; the unhandled-exception filter reports and terminates.
            #[cfg(windows)]
            // SAFETY: RaiseException with no arguments; control never returns because
            // the top-level filter executes the terminating handler.
            unsafe {
                windows_sys::Win32::System::Diagnostics::Debug::RaiseException(
                    0xC000_0005,
                    0,
                    0,
                    std::ptr::null(),
                );
            }
            std::process::abort()
        }
        "hang" => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
        _ => panic!("play.debug.crash requested a panic"),
    }
}
