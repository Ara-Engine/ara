//! Converter descriptor tables: the bindless heap of the Metal backend. Each table is
//! one shared buffer of [`TableEntry`] records; the top-level argument buffer carries
//! the table addresses. Slots are immutable while referenced: a slot is written once
//! when published and freed only after its resource retires on both timelines. The two sampler tables cache immutable regular/comparison states.
use super::abi::{self, ENTRY_BYTES, TableEntry};
use super::native::nil;
use crate::{HeapIndex, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBuffer, MTLDevice, MTLResource, MTLResourceOptions, MTLSamplerAddressMode,
    MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLSamplerState,
};

/// One table: a shared entry array plus monotonic slot allocation with reuse.
pub(super) struct Table {
    pub buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    /// The heap name in exhaustion errors, as the Vulkan backend reports it.
    heap: &'static str,
    capacity: u32,
    next: u32,
    free: Vec<u32>,
    live: u32,
}

impl Table {
    fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        capacity: u32,
        label: &str,
        heap: &'static str,
    ) -> Result<Self> {
        let buffer = device
            .newBufferWithLength_options(
                (u64::from(capacity) * ENTRY_BYTES) as usize,
                MTLResourceOptions::StorageModeShared,
            )
            .ok_or_else(|| nil(label))?;
        buffer.setLabel(Some(&objc2_foundation::NSString::from_str(label)));
        Ok(Self {
            buffer,
            heap,
            capacity,
            next: 0,
            free: Vec::new(),
            live: 0,
        })
    }
    /// Publish an entry in a fresh (or fully retired) slot.
    pub fn insert(&mut self, entry: TableEntry) -> Result<HeapIndex> {
        let index = match self.free.pop() {
            Some(index) => index,
            None if self.next < self.capacity => {
                self.next += 1;
                self.next - 1
            }
            None => {
                return Err(crate::GpuError::Contract(format!(
                    "{} heap exhausted ({} slots, including pending retirement)",
                    self.heap, self.capacity
                )));
            }
        };
        let entries = self.buffer.contents().cast::<TableEntry>();
        // SAFETY: `index < capacity` and the buffer holds `capacity` entries in shared
        // storage. The slot is fresh or retired on every timeline, so the GPU is not
        // reading it; the write is visible to later submissions (coherent memory).
        unsafe { entries.add(index as usize).write(entry) };
        self.live += 1;
        Ok(HeapIndex { index })
    }
    /// Return a slot whose resource completed on every queue.
    pub fn release(&mut self, index: HeapIndex) {
        self.free.push(index.index);
        self.live -= 1;
    }
    /// Slots currently referenced by live resources.
    pub fn live(&self) -> u32 {
        self.live
    }
}

fn sampler_state(
    device: &ProtocolObject<dyn MTLDevice>,
    desc: &crate::sampler::SamplerDesc,
) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>> {
    use crate::sampler::{AddressMode as A, Filter as F};
    desc.validate()?;
    let filter = |f| match f {
        F::Nearest => MTLSamplerMinMagFilter::Nearest,
        F::Linear => MTLSamplerMinMagFilter::Linear,
    };
    let address = |a| match a {
        A::Repeat => MTLSamplerAddressMode::Repeat,
        A::MirrorRepeat => MTLSamplerAddressMode::MirrorRepeat,
        A::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
    };
    let d = MTLSamplerDescriptor::new();
    d.setMinFilter(filter(desc.min));
    d.setMagFilter(filter(desc.mag));
    d.setMipFilter(match desc.mip {
        F::Nearest => MTLSamplerMipFilter::Nearest,
        F::Linear => MTLSamplerMipFilter::Linear,
    });
    d.setSAddressMode(address(desc.address[0]));
    d.setTAddressMode(address(desc.address[1]));
    d.setRAddressMode(address(desc.address[2]));
    d.setMaxAnisotropy(desc.anisotropy as usize);
    d.setLodMaxClamp(f32::MAX);
    if let Some(compare) = desc.compare {
        d.setCompareFunction(super::native::compare(compare));
    }
    d.setSupportArgumentBuffers(true);
    device
        .newSamplerStateWithDescriptor(&d)
        .ok_or_else(|| nil("sampler"))
}

/// The four root-signature tables, in root-parameter order, then the static samplers.
pub(super) struct Tables {
    pub textures: Table,
    pub buffers: Table,
    pub storage_images: Table,
    pub accelerations: Table,
    samplers: Table,
    comparison_samplers: Table,
    /// Kept alive for the sampler table's resource IDs.
    comparison_states: Vec<(
        crate::sampler::SamplerDesc,
        Retained<ProtocolObject<dyn MTLSamplerState>>,
    )>,
    sampler_states: Vec<(
        crate::sampler::SamplerDesc,
        Retained<ProtocolObject<dyn MTLSamplerState>>,
    )>,
}

impl Tables {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        let sampler_states = crate::sampler::SamplerDesc::defaults()
            .into_iter()
            .map(|desc| sampler_state(device, &desc).map(|state| (desc, state)))
            .collect::<Result<Vec<_>>>()?;
        let mut samplers = Table::new(device, abi::SAMPLERS, "ara.samplers", "Sampler")?;
        for (_, state) in &sampler_states {
            samplers.insert(TableEntry::sampler(state.gpuResourceID().to_raw()))?;
        }
        Ok(Self {
            textures: Table::new(device, abi::TEXTURES, "ara.textures", "Sampled")?,
            buffers: Table::new(device, abi::BUFFERS, "ara.buffers", "Buffer")?,
            storage_images: Table::new(
                device,
                abi::STORAGE_IMAGES,
                "ara.storage_images",
                "Storage-image",
            )?,
            accelerations: Table::new(
                device,
                abi::ACCELERATIONS,
                "ara.accelerations",
                "Acceleration",
            )?,
            samplers,
            comparison_samplers: Table::new(
                device,
                abi::SAMPLERS,
                "ara.comparison_samplers",
                "Comparison sampler",
            )?,
            comparison_states: Vec::new(),
            sampler_states,
        })
    }
    pub fn sampler(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        desc: &crate::sampler::SamplerDesc,
    ) -> Result<HeapIndex> {
        desc.validate()?;
        let (table, states) = if desc.compare.is_some() {
            (&mut self.comparison_samplers, &mut self.comparison_states)
        } else {
            (&mut self.samplers, &mut self.sampler_states)
        };
        if let Some(index) = states.iter().position(|(d, _)| d == desc) {
            return Ok(HeapIndex {
                index: index as u32,
            });
        }
        let state = sampler_state(device, desc)?;
        let slot = table.insert(TableEntry::sampler(state.gpuResourceID().to_raw()))?;
        states.push((*desc, state));
        Ok(slot)
    }
    /// Table addresses for a top-level argument buffer.
    pub fn top_level(&self, root: [u32; 8]) -> abi::TopLevel {
        abi::TopLevel {
            root,
            textures: self.textures.buffer.gpuAddress(),
            buffers: self.buffers.buffer.gpuAddress(),
            storage_images: self.storage_images.buffer.gpuAddress(),
            accelerations: self.accelerations.buffer.gpuAddress(),
            samplers: self.samplers.buffer.gpuAddress(),
            comparison_samplers: self.comparison_samplers.buffer.gpuAddress(),
        }
    }
    /// Every table buffer, for the residency set.
    pub fn buffers(&self) -> [&ProtocolObject<dyn MTLBuffer>; 6] {
        [
            &self.textures.buffer,
            &self.buffers.buffer,
            &self.storage_images.buffer,
            &self.accelerations.buffer,
            &self.samplers.buffer,
            &self.comparison_samplers.buffer,
        ]
    }
}
