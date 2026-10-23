//! Tick-thread priority while the compile server compiles (decision A13, S2 finding 5).
//!
//! Threads that run ticks (the tick thread and the compute task pool) register once
//! with [`register_tick_thread`]; while a compile is in flight they run at above-normal
//! priority, which kept tick p99 inside reload windows at 23 ms instead of 43 ms in S2.
//! The compile server also lowers its own process priority, which is the cross-platform
//! half; raising thread priority is implemented on Windows.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Thread handles (as integers) registered for boosting.
static THREADS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Registers the calling thread for boosting. Call it from the tick thread and from
/// the compute task pool's `on_thread_spawn` hook. Idempotent per thread.
pub fn register_tick_thread() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThreadId, OpenThread, THREAD_QUERY_INFORMATION, THREAD_SET_INFORMATION,
        };
        thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
        if DONE.with(|d| d.replace(true)) {
            return;
        }
        // SAFETY: plain Win32 calls on the current thread's id. The handle is never closed:
        // registered threads (tick thread, task pool) live as long as the process.
        let handle = unsafe {
            OpenThread(
                THREAD_SET_INFORMATION | THREAD_QUERY_INFORMATION,
                0,
                GetCurrentThreadId(),
            )
        };
        if !handle.is_null()
            && let Ok(mut threads) = THREADS.lock()
        {
            threads.push(handle as usize);
        }
    }
}

/// Switches registered threads between normal and above-normal priority.
#[derive(Default)]
pub(crate) struct Boost {
    active: AtomicBool,
    disabled: AtomicBool,
}

impl Boost {
    /// Turns boosting off entirely (the `ScriptConfig::boost_ticks` switch).
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::Relaxed);
    }

    /// Raises (`true`) or restores (`false`) registered threads' priority.
    pub fn set(&self, active: bool) {
        if self.disabled.load(Ordering::Relaxed)
            || self.active.swap(active, Ordering::AcqRel) == active
        {
            return;
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{
                SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_NORMAL,
            };
            let priority = if active {
                THREAD_PRIORITY_ABOVE_NORMAL
            } else {
                THREAD_PRIORITY_NORMAL
            };
            if let Ok(threads) = THREADS.lock() {
                for &h in threads.iter() {
                    // SAFETY: `h` is a thread handle opened by `register_tick_thread` with
                    // THREAD_SET_INFORMATION and never closed.
                    unsafe {
                        SetThreadPriority(h as windows_sys::Win32::Foundation::HANDLE, priority)
                    };
                }
            }
        }
    }

    /// True while a compile is in flight.
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
}
