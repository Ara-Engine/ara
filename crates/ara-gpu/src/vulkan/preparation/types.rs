use super::*;

enum DataSource {
    Bytes(Vec<u8>),
    Deferred(Box<dyn FnOnce() -> Result<Vec<u8>> + Send>),
}

/// Initial bytes, already prepared or produced on the allocation worker. A
/// deferred source can stage compressed mip data without copying it on the UI.
pub struct InitialData(DataSource);

impl Default for InitialData {
    fn default() -> Self {
        Self(DataSource::Bytes(Vec::new()))
    }
}

impl From<Vec<u8>> for InitialData {
    fn from(bytes: Vec<u8>) -> Self {
        Self(DataSource::Bytes(bytes))
    }
}

impl InitialData {
    /// Runs once on the worker after admission; requires a host-visible buffer.
    pub fn deferred(build: impl FnOnce() -> Result<Vec<u8>> + Send + 'static) -> Self {
        Self(DataSource::Deferred(Box::new(build)))
    }

    pub(crate) fn is_empty(&self) -> bool {
        matches!(&self.0, DataSource::Bytes(bytes) if bytes.is_empty())
    }
    pub(crate) fn load(self) -> Result<Vec<u8>> {
        match self.0 {
            DataSource::Bytes(bytes) => Ok(bytes),
            DataSource::Deferred(build) => build(),
        }
    }
}

/// Unpublished native resources to allocate and initialize off the owner thread.
/// These contain no commands or queue access. Keep GPU uploads ordered through
/// the normal submit interface after adoption.
#[derive(Default)]
pub struct ResourcePreparation {
    /// Buffer allocations in the order returned by `PreparedResources::buffers`.
    pub buffers: Vec<BufferPreparation>,
    /// Image allocations in the order returned by `PreparedResources::images`.
    pub images: Vec<ImagePreparation>,
}

/// A buffer and optional initial coherent host data, moved to the worker once.
pub struct BufferPreparation {
    /// Nonzero capacity in bytes.
    pub size: u64,
    /// Native uses, matching `VulkanDevice::create_buffer`.
    pub usage: vk::BufferUsageFlags,
    /// Coherent host memory; false selects device-local memory.
    pub host: bool,
    /// Initial data at offset zero; must fit. Deferred/nonempty data requires `host`.
    pub data: InitialData,
}

/// An initially undefined image, matching `VulkanDevice::create_mip_image`.
pub struct ImagePreparation {
    /// Physical pixel extent.
    pub size: [u32; 2],
    /// Native storage/sampling format.
    pub format: vk::Format,
    /// Finest-first resident mip count.
    pub mip_levels: u32,
    /// Declared native image uses.
    pub usage: vk::ImageUsageFlags,
}

/// Prepared resources adopted by their creating device. No descriptor slots are
/// allocated until explicitly bound. Retire the handles normally when finished.
#[must_use]
pub struct PreparedResources {
    /// Initialized buffers in request order; populated host data starts HostWrite.
    pub buffers: Vec<Buffer>,
    /// Images in request order; initialize them through declared upload/clear passes.
    pub images: Vec<Image>,
}

/// Device-local preparation identity; cancellation expires it immediately.
pub enum PreparationKind {}
/// Generation-checked request identity, distinct from adopted resource handles.
pub type PreparationId = crate::arena::Handle<PreparationKind>;
