//! Test support: hold one native queue busy until the host releases it, and the
//! device-teardown exclusion that makes that safe with other devices in the process.
use super::Context;
use anyhow::Result;
use ash::vk::{self, TaggedStructure};
use std::sync::{Condvar, Mutex};

/// Queue gates held (closed) anywhere in the process. Device teardown waits for zero.
static HELD: Mutex<usize> = Mutex::new(0);
/// Notified whenever a gate opens.
static OPENED: Condvar = Condvar::new();

/// Runs `teardown` (a device's `vkDestroyDevice` and the instance teardown around
/// it) once no [`QueueGate`] is held anywhere in the process; gates closing meanwhile
/// wait for it to finish.
///
/// The NVIDIA Windows driver's `vkDestroyDevice` spins while holding a process-wide
/// driver lock for as long as any queue of another device is held on a host action (a
/// host-set event or a host-signalled semaphore). The holder's next driver call then
/// blocks on that lock, so it can never release its queue: a deadlock (reproduced by
/// `ara-render`'s live and GI tests in parallel, see `crates/ara-render/AGENTS.md`).
/// Engine code never holds a queue on the host, so outside tests this never waits.
pub(super) fn without_held_queues<R>(teardown: impl FnOnce() -> R) -> R {
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    while *held > 0 {
        held = OPENED.wait(held).unwrap_or_else(|e| e.into_inner());
    }
    // The lock stays held through teardown, so no gate closes during it.
    teardown()
}

/// Blocks a native queue of a device until [`QueueGate::open`] (or drop), so tests can
/// observe a frame that is still in flight. Later submissions on that queue queue up
/// behind it; other queues keep running.
///
/// The block is a submission waiting on a host-signalled timeline semaphore (a valid
/// wait-before-signal; a host-set `VkEvent` waited on the GPU is not, since
/// `VUID-vkCmdWaitEvents2-pEvents-03840`). While any gate is held, device teardown
/// elsewhere in the process waits ([`without_held_queues`]), so a thread holding a
/// gate must not drop a device (its own included) before opening it: declare the gate
/// after its renderer.
///
/// Only the thread that owns the queue's submissions may create, open or drop it.
pub struct QueueGate {
    device: ash::Device,
    semaphore: vk::Semaphore,
    opened: bool,
}

impl QueueGate {
    /// Submits the blocking wait on `queue`, one of `context`'s native queues
    /// (`graphics`, `scene_graphics` or `compute`).
    pub fn new(context: &Context, queue: vk::Queue) -> Result<Self> {
        let device = context.device.clone();
        let mut kind =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        // SAFETY: The create info lives through the call; the gate owns the semaphore
        // until Drop has signalled it and drained the device.
        let semaphore = unsafe {
            device.create_semaphore(&vk::SemaphoreCreateInfo::default().push(&mut kind), None)?
        };
        // Counted before the queue is held, after any teardown in progress. From here
        // Drop signals, uncounts and destroys, also when the submission fails.
        *HELD.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        let gate = Self {
            device,
            semaphore,
            opened: false,
        };
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(semaphore)
            .value(1)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        // SAFETY: The caller owns `queue`'s submissions on this thread; the wait names
        // the gate's own semaphore, which only `open` signals.
        unsafe {
            gate.device.queue_submit2(
                queue,
                &[vk::SubmitInfo2::default().wait_semaphore_infos(&waits)],
                vk::Fence::null(),
            )?;
        }
        Ok(gate)
    }

    /// Releases the queue: the blocked wait and everything behind it run.
    pub fn open(&mut self) -> Result<()> {
        if !self.opened {
            // SAFETY: The gate's own timeline; this is its only signal.
            unsafe {
                self.device.signal_semaphore(
                    &vk::SemaphoreSignalInfo::default()
                        .semaphore(self.semaphore)
                        .value(1),
                )?;
            }
            self.opened = true;
            release();
        }
        Ok(())
    }
}

/// Uncounts one held gate and wakes waiting teardowns.
fn release() {
    *HELD.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
    OPENED.notify_all();
}

impl Drop for QueueGate {
    fn drop(&mut self) {
        if self.open().is_err() && !self.opened {
            // The signal failed (a lost device): uncount it anyway, so teardown
            // elsewhere is not held by a gate that can never open.
            self.opened = true;
            release();
        }
        // SAFETY: Signalling precedes the drain (also after a failed assertion), and
        // the drain precedes destroying the semaphore its wait names.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_semaphore(self.semaphore, None);
        }
    }
}
