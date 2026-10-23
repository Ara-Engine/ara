//! Diagnostic-only direct Jolt comparison; never linked by ordinary game builds.
use crate::JoltBackend;
use ara_jolt_sys as sys;
use ara_physics::{PhysicsError, Result};

unsafe extern "C" {
    fn ara_bench_character_tree(world: *mut sys::AraWorld, enabled: u32);
    fn ara_bench_native_step(world: *mut sys::AraWorld, dt: f32) -> u32;
    fn ara_bench_reset(world: *mut sys::AraWorld);
    fn ara_bench_stats(world: *const sys::AraWorld, out: *mut u64);
}

impl JoltBackend {
    /// Selects the original all-character scan for an identical-state A/B test.
    pub fn benchmark_character_tree(&mut self, enabled: bool) {
        // SAFETY: exclusive live world, all character checks are synchronous.
        unsafe { ara_bench_character_tree(self.ptr(), u32::from(enabled)) };
    }
    /// Direct C++ PhysicsSystem::Update, with identical world/allocator/job settings.
    /// Requires contact collection and virtual characters to be absent. This does
    /// not advance PhysicsWorld's tick and is only for a standalone benchmark world.
    pub fn benchmark_native_step(&mut self, dt: f32) -> Result<()> {
        // SAFETY: exclusive world access, scoped host workers, and native validation
        // enforce the restricted rigid-body benchmark contract.
        let status = self.run_jobs(|world| unsafe { ara_bench_native_step(world, dt) });
        if status == 0 {
            Ok(())
        } else {
            Err(PhysicsError(format!(
                "native benchmark failed: {}",
                Self::error()
            )))
        }
    }
    /// Clears diagnostic counters before one isolated update. Do not overlap worlds.
    pub fn benchmark_reset(&mut self) {
        // SAFETY: exclusively owned world; no workers exist outside run_jobs scopes.
        unsafe { ara_bench_reset(self.ptr()) };
    }
    /// Exclusive CPU ns for broadphase, narrowphase, setup, velocity, position,
    /// integration, islands, bookkeeping; then native wall ns, arena peak, fallbacks.
    /// Phase counters require `profile`; ordinary `bench` builds return zero phases.
    pub fn benchmark_stats(&self) -> [u64; 11] {
        let mut stats = [0; 11];
        // SAFETY: exactly eleven writable u64 values, with no concurrent update.
        unsafe { ara_bench_stats(self.ptr(), stats.as_mut_ptr()) };
        stats
    }
}
