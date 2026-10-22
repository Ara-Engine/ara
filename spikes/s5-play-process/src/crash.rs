//! Player-side crash reporting and the deliberate crash hooks used by the isolation test.
//!
//! Each reporter writes one `ARA_PLAYER_CRASH {json}` line to stderr; the host captures the
//! player's stderr and turns that line plus the exit code into a structured error. A production
//! player would write a minidump from an out-of-process handler instead (A11: Sentry).

use std::io::Write;
use std::time::Duration;

use serde_json::json;

use crate::protocol::CrashKind;

/// Prefix of the structured crash line on stderr.
pub const CRASH_PREFIX: &str = "ARA_PLAYER_CRASH ";

/// Installs the panic hook and, on Windows, the unhandled-exception filter. Also disables the
/// Windows Error Reporting dialog, which would otherwise keep a crashed player alive.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_owned());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let thread = std::thread::current().name().map(str::to_owned);
        let line =
            json!({ "kind": "panic", "message": message, "location": location, "thread": thread });
        let _ = writeln!(std::io::stderr(), "{CRASH_PREFIX}{line}");
    }));
    #[cfg(windows)]
    windows::install();
}

/// Crashes or hangs on purpose. Only reachable from `play.debug.crash`.
pub fn trigger(kind: CrashKind) -> ! {
    match kind {
        CrashKind::Panic => panic!("forced panic from play.debug.crash"),
        CrashKind::AccessViolation => {
            // A non-null address in the never-mapped first page, hidden from the optimizer so
            // the store is really emitted.
            let addr = std::hint::black_box(0x10usize) as *mut u32;
            // SAFETY: deliberately unsound. The write faults with an access violation, which is
            // the behaviour under test; the process never continues past it.
            unsafe { addr.write_volatile(0xDEAD) };
            unreachable!("the write above faults")
        }
        CrashKind::Hang => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
    }
}

#[cfg(windows)]
mod windows {
    use std::io::Write;

    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_EXECUTE_HANDLER, EXCEPTION_POINTERS, SEM_FAILCRITICALERRORS,
        SEM_NOGPFAULTERRORBOX, SetErrorMode, SetUnhandledExceptionFilter,
    };

    use super::CRASH_PREFIX;

    pub fn install() {
        // SAFETY: both calls only change process-wide error-handling settings, and `filter`
        // matches the required `extern "system"` signature.
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
            SetUnhandledExceptionFilter(Some(filter));
        }
    }

    fn name(code: u32) -> &'static str {
        match code {
            0xC000_0005 => "STATUS_ACCESS_VIOLATION",
            0xC000_00FD => "STATUS_STACK_OVERFLOW",
            0xC000_001D => "STATUS_ILLEGAL_INSTRUCTION",
            0xC000_0094 => "STATUS_INTEGER_DIVIDE_BY_ZERO",
            _ => "UNKNOWN_EXCEPTION",
        }
    }

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the OS passes a valid EXCEPTION_POINTERS whose record outlives this call.
        let record = unsafe { &*(*info).ExceptionRecord };
        let code = record.ExceptionCode as u32;
        // Formatting allocates inside a crashed process. Acceptable for a spike; production
        // reporting runs out of process.
        let line = format!(
            "{CRASH_PREFIX}{{\"kind\":\"exception\",\"code\":\"0x{code:08X}\",\"name\":\"{}\",\"address\":\"{:p}\"}}\n",
            name(code),
            record.ExceptionAddress
        );
        let _ = std::io::stderr().write_all(line.as_bytes());
        // Terminate now, with the exception code as the exit code, and without WER.
        EXCEPTION_EXECUTE_HANDLER
    }
}
