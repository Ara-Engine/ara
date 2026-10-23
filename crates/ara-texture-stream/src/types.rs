use ara_gpu::{Access, Buffer, Resource};
/// Immutable sampling handles for mesh, landscape and other material graphs.
#[derive(Clone, Copy)]
pub struct Binding {
    /// Descriptor, page table, physical pool and feedback heap slots.
    pub slots: [u32; 4],
    /// Frame epoch, diagnostics slot, viewport width and height.
    pub tracking: [u32; 4],
    pub(crate) resources: [Buffer; 5],
    pub(crate) textures: u32,
}
impl Binding {
    /// Valid virtual texture-ID range is 0..texture_count.
    pub fn texture_count(self) -> u32 {
        self.textures
    }
    /// Complete bindless reachability; all accesses occur on Graphics. The first
    /// three are ShaderRead, feedback/diagnostics are Storage.
    pub fn resources(self) -> [(Resource, Access); 5] {
        std::array::from_fn(|i| {
            (
                Resource::Buffer(self.resources[i]),
                if i < 3 {
                    Access::ShaderRead
                } else {
                    Access::Storage
                },
            )
        })
    }
}
/// Capture counters for budget, actual sampling and desired/resident mip error.
#[derive(Default, serde::Serialize)]
pub struct Stats {
    /// Total virtual tiles across all mip chains.
    pub virtual_pages: u32,
    /// Physical tiles holding valid data, including pinned mip tails.
    pub resident_pages: u32,
    /// Physical allocation including gutters; bounded by the requested budget.
    pub pool_bytes: u64,
    /// Page tables, feedback, diagnostics and the bounded upload staging ring.
    pub auxiliary_bytes: u64,
    /// Complete source mip bytes; excludes CPU allocator overhead.
    pub source_bytes: u64,
    /// Pages transferred in the most recently prepared frame.
    pub uploaded_pages: u32,
    /// GPU samples in the completed frame.
    pub samples: u32,
    /// Samples with desired-to-resident lag greater than one mip.
    pub samples_over_one_mip: u32,
    /// Maximum actual minus desired mip in the completed frame.
    pub max_mip_lag: f32,
}
