//! Converter descriptor tables: the bindless heap of the Metal backend. Each table is
//! one shared buffer of [`TableEntry`] records; the top-level argument buffer carries
//! the table addresses. Slots are immutable while referenced: a slot is written once
//! when published and freed only after its resource retires on both timelines.
use super::abi::{self, ENTRY_BYTES, TableEntry};
use super::native::nil;
use crate::{HeapIndex, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLBuffer, MTLDevice, MTLResource, MTLResourceOptions};

/// One table: a shared entry array plus monotonic slot allocation with reuse.
pub(super) struct Table {
    pub buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    capacity: u32,
    next: u32,
    free: Vec<u32>,
    live: u32,
}

impl Table {
    fn new(device: &ProtocolObject<dyn MTLDevice>, capacity: u32, label: &str) -> Result<Self> {
        let buffer = device
            .newBufferWithLength_options(
                (u64::from(capacity) * ENTRY_BYTES) as usize,
                MTLResourceOptions::StorageModeShared,
            )
            .ok_or_else(|| nil(label))?;
        buffer.setLabel(Some(&objc2_foundation::NSString::from_str(label)));
        Ok(Self {
            buffer,
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
                    "Descriptor table capacity {} exhausted",
                    self.capacity
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

/// The four root-signature tables, in root-parameter order.
pub(super) struct Tables {
    pub textures: Table,
    pub buffers: Table,
    pub storage_images: Table,
    pub accelerations: Table,
}

impl Tables {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        Ok(Self {
            textures: Table::new(device, abi::TEXTURES, "ara.textures")?,
            buffers: Table::new(device, abi::BUFFERS, "ara.buffers")?,
            storage_images: Table::new(device, abi::STORAGE_IMAGES, "ara.storage_images")?,
            accelerations: Table::new(device, abi::ACCELERATIONS, "ara.accelerations")?,
        })
    }
    /// Table addresses for a top-level argument buffer.
    pub fn top_level(&self, root: [u32; 8]) -> abi::TopLevel {
        abi::TopLevel {
            root,
            textures: self.textures.buffer.gpuAddress(),
            buffers: self.buffers.buffer.gpuAddress(),
            storage_images: self.storage_images.buffer.gpuAddress(),
            accelerations: self.accelerations.buffer.gpuAddress(),
        }
    }
    /// Every table buffer, for the residency set.
    pub fn buffers(&self) -> [&ProtocolObject<dyn MTLBuffer>; 4] {
        [
            &self.textures.buffer,
            &self.buffers.buffer,
            &self.storage_images.buffer,
            &self.accelerations.buffer,
        ]
    }
}
