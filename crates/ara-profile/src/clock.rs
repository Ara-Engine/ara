use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Counter ticks stay in the producer; nanosecond conversion stays in collection.
pub(crate) struct Clock {
    origin: Instant,
    epoch: u64,
    tsc: bool,
    nanos_per_tick: AtomicU64,
}
impl Clock {
    pub fn new() -> Self {
        let tsc = invariant_tsc();
        let origin = Instant::now();
        let clock = Self {
            origin,
            epoch: if tsc { counter() } else { 0 },
            tsc,
            nanos_per_tick: AtomicU64::new(1 << 32),
        };
        if tsc {
            // Cold setup only. Avoid estimating frequency from a few QPC ticks.
            std::thread::sleep(Duration::from_millis(2));
            clock.calibrate();
        }
        clock
    }
    pub fn now(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
    }
    /// One reference-clock sample per CPU frame; invariant frequency is refined
    /// over the entire capture rather than a noisy, possibly tiny frame interval.
    pub fn calibrate(&self) -> u64 {
        let nanos = self.now();
        if self.tsc {
            let ticks = counter().saturating_sub(self.epoch);
            if ticks > 0 {
                let scale = ((u128::from(nanos) << 32) / u128::from(ticks)) as u64;
                self.nanos_per_tick.store(scale, Ordering::Relaxed);
            }
        }
        nanos
    }
    #[inline(always)]
    pub fn read(&self) -> u64 {
        if self.tsc { counter() } else { self.now() }
    }
    pub fn duration(&self, ticks: u64) -> u64 {
        if self.tsc {
            ((u128::from(ticks) * u128::from(self.nanos_per_tick.load(Ordering::Relaxed))) >> 32)
                .min(u128::from(u64::MAX)) as u64
        } else {
            ticks
        }
    }
    pub fn timestamp(&self, ticks: u64) -> u64 {
        self.duration(ticks.saturating_sub(self.epoch))
    }
    pub fn source(&self) -> &'static str {
        if self.tsc {
            "invariant-tsc"
        } else {
            "monotonic"
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn invariant_tsc() -> bool {
    // CPUID is available on x86-64; check the extended leaf before using it.
    std::arch::x86_64::__cpuid(0x8000_0000).eax >= 0x8000_0007
        && std::arch::x86_64::__cpuid(0x8000_0007).edx & (1 << 8) != 0
}
#[cfg(not(target_arch = "x86_64"))]
fn invariant_tsc() -> bool {
    false
}

#[inline(always)]
fn counter() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        // Like Tracy's counter path, these are relaxed timestamp reads, not
        // instruction-serialization barriers. Keep compiler movement bounded.
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
        // SAFETY: RDTSC is available on x86-64; used only after invariant-TSC
        // detection. It does not access memory or require pointer validity.
        let ticks = unsafe { std::arch::x86_64::_rdtsc() };
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
        ticks
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrated_ticks_track_the_reference_clock() {
        let clock = Clock::new();
        let wall = Instant::now();
        let before = clock.read();
        std::thread::sleep(Duration::from_millis(5));
        let after = clock.read();
        let elapsed = wall.elapsed().as_nanos() as u64;
        clock.calibrate();
        let measured = clock.duration(after.saturating_sub(before));
        assert!(measured.abs_diff(elapsed) < elapsed / 10 + 10_000);
    }
}
