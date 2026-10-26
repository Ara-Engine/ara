//! Bounded off-thread native allocation, with adoption on the device owner.
use super::*;
use context::allocation;
mod raw;
mod types;
mod worker;
pub use types::*;
pub(super) use worker::Worker;

impl VulkanDevice {
    /// Queue allocation/initial host copies off-thread. At most two requests are
    /// retained (one running, one queued or awaiting adoption). `build` is called
    /// only when admitted; None lets a control-boundary caller keep its input.
    /// The worker never submits commands or accesses the owner's heap/pools.
    pub fn prepare_resources(
        &mut self,
        build: impl FnOnce() -> ResourcePreparation,
    ) -> Result<Option<PreparationId>> {
        if self.preparation.is_none() {
            self.preparation = Some(Worker::new(self.context.allocator())?);
        }
        let disposal = self.disposal.as_mut().expect("disposal worker alive");
        self.preparation
            .as_mut()
            .expect("preparation worker initialized")
            .submit(disposal, build)
    }

    /// Poll without waiting. A completed request transfers sole native ownership
    /// into this device and expires its ticket. Partial failures adopt nothing.
    pub fn poll_prepared_resources(
        &mut self,
        id: PreparationId,
    ) -> Result<Option<PreparedResources>> {
        let result = self
            .preparation
            .as_mut()
            .ok_or(GpuError::StaleHandle)?
            .poll(self.disposal.as_mut().expect("disposal worker alive"), id)?;
        result.map(|result| result?.adopt(self)).transpose()
    }

    /// Cancel without joining or destroying driver objects on the caller. A
    /// running allocation finishes unseen and is disposed without entering arenas.
    pub fn cancel_prepared_resources(&mut self, id: PreparationId) -> Result<()> {
        self.preparation
            .as_mut()
            .ok_or(GpuError::StaleHandle)?
            .cancel(self.disposal.as_mut().expect("disposal worker alive"), id)
    }
}
