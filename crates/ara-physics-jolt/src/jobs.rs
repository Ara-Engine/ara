use crate::JoltBackend;
use ara_jolt_sys as sys;
use ara_physics::{PhysicsConfig, PhysicsError, Result};
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};

pub(crate) fn validate_pool(config: &PhysicsConfig) -> Result<()> {
    if config.threads > 64 {
        return Err(PhysicsError(
            "Jolt supports at most 64 simulation threads".into(),
        ));
    }
    if config.shared_job_pool && config.threads > 1 {
        let pool = ComputeTaskPool::get_or_init(|| {
            TaskPoolBuilder::new()
                .num_threads(config.threads as usize - 1)
                .build()
        });
        if pool.thread_num() == 0 {
            return Err(PhysicsError(
                "shared Jolt jobs require at least one compute worker".into(),
            ));
        }
    }
    Ok(())
}

impl JoltBackend {
    /// Borrow host workers for this boundary. The native queue owns job references;
    /// scoped tasks cannot outlive the world. No native callback enters Rust.
    pub(crate) fn run_jobs(&mut self, step: impl FnOnce(*mut sys::AraWorld) -> u32) -> u32 {
        if !self.config.shared_job_pool || self.config.threads <= 1 {
            return step(self.ptr());
        }
        let workers = self.config.threads as usize - 1;
        let pool = ComputeTaskPool::get();
        let address = self.ptr() as usize;
        let workers = workers.min(pool.thread_num());
        // SAFETY: host job mode was selected at construction; the preceding scope
        // has joined every worker before the next boundary can begin.
        unsafe { sys::ara_jobs_begin(self.ptr(), workers as u32) };
        let mut status = 0;
        pool.scope(|scope| {
            for _ in 0..workers {
                scope.spawn(async move {
                    // SAFETY: exclusive stepping scope lends only the job queue
                    // to workers; all tasks join before that borrow ends.
                    unsafe { sys::ara_jobs_run(address as *mut sys::AraWorld) };
                });
            }
            status = step(self.ptr());
            // SAFETY: Update has completed, including its barrier. Wake workers
            // to drain queued references and return before releasing the world.
            unsafe { sys::ara_jobs_end(self.ptr()) };
        });
        status
    }
}
