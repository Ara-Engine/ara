//! Event-aware frame waits, independent of the coarse Windows message timeout.
use std::time::Duration;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED},
        System::Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimer,
            TIMER_ALL_ACCESS,
        },
        UI::WindowsAndMessaging::{MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, QS_ALLINPUT},
    },
    core::PCWSTR,
};

pub(super) struct FrameTimer(HANDLE);
impl FrameTimer {
    pub fn new() -> anyhow::Result<Self> {
        // SAFETY: Unnamed, auto-reset timer, owned exclusively by this window thread.
        Ok(Self(unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )?
        }))
    }
    pub fn wait(&self, remaining: Duration) -> anyhow::Result<()> {
        // Preserve the SDL timeout's whole-millisecond rounding, leaving scheduling headroom.
        let ticks = remaining.as_millis().min((i64::MAX / 10_000) as u128) as i64 * 10_000;
        if ticks == 0 {
            return Ok(());
        }
        let due = -ticks;
        // SAFETY: The timer is live; a negative due time is relative. No APC or borrowed callback data.
        unsafe {
            SetWaitableTimer(self.0, &due, 0, None, None, false)?;
        }
        // SAFETY: One live owned handle. SDL pumps queued window/input messages on the next frame.
        let result = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&[self.0]), u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if result == WAIT_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}
impl Drop for FrameTimer {
    fn drop(&mut self) {
        // SAFETY: This is the sole owner and no wait remains active when the event loop exits.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
