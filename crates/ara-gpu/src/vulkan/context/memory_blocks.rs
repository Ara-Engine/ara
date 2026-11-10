//! Owned device-memory blocks. Buffer/image classes are segregated to satisfy
//! bufferImageGranularity without padding every allocation to that granularity.
use crate::{GpuError, Result};
use ash::vk::{self, TaggedStructure};
use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Buffer,
    Image,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Key {
    pub memory_type: u32,
    pub address: bool,
    pub class: Class,
}
/// Unique block identity prevents late disposal replies from aliasing reused handles.
#[derive(Clone, Copy)]
pub(crate) struct Slice {
    pub id: u64,
    pub memory: vk::DeviceMemory,
    pub offset: u64,
    pub size: u64,
}
#[derive(Default)]
struct Ranges {
    free: Vec<(u64, u64)>,
    used: u64,
}
impl Ranges {
    fn new(size: u64) -> Self {
        Self {
            free: vec![(0, size)],
            used: 0,
        }
    }
    fn take(&mut self, size: u64, align: u64) -> Option<u64> {
        for i in 0..self.free.len() {
            let (start, len) = self.free[i];
            let at = start.checked_next_multiple_of(align)?;
            let end = at.checked_add(size)?;
            if end > start + len {
                continue;
            }
            self.free.swap_remove(i);
            if at > start {
                self.free.push((start, at - start));
            }
            if end < start + len {
                self.free.push((end, start + len - end));
            }
            self.used += size;
            return Some(at);
        }
        None
    }
    fn release(&mut self, offset: u64, size: u64) {
        assert!(self.used >= size, "each allocated range retires once");
        self.used -= size;
        self.free.push((offset, size));
        self.free.sort_unstable_by_key(|r| r.0);
        let mut i = 0;
        while i + 1 < self.free.len() {
            if self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
                let next = self.free.remove(i + 1);
                self.free[i].1 += next.1;
            } else {
                i += 1;
            }
        }
    }
}
struct Block {
    id: u64,
    memory: vk::DeviceMemory,
    size: u64,
    key: Key,
    ranges: Ranges,
}
pub(crate) struct MemoryBlocks {
    device: Option<Box<ash::Device>>,
    blocks: Vec<Block>,
    minimum: u64,
}
impl Default for MemoryBlocks {
    fn default() -> Self {
        Self {
            device: None,
            blocks: Vec::new(),
            minimum: 8 * 1024 * 1024,
        }
    }
}
impl MemoryBlocks {
    pub fn small() -> Self {
        Self {
            device: None,
            blocks: Vec::new(),
            minimum: 65536,
        }
    }
    pub fn allocate(
        &mut self,
        device: &ash::Device,
        key: Key,
        requirements: vk::MemoryRequirements,
    ) -> Result<Slice> {
        for block in &mut self.blocks {
            if block.key != key {
                continue;
            }
            if let Some(offset) = block.ranges.take(requirements.size, requirements.alignment) {
                return Ok(Slice {
                    id: block.id,
                    memory: block.memory,
                    offset,
                    size: requirements.size,
                });
            }
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| GpuError::Contract("Memory block identities exhausted".into()))?;
        let size = requirements
            .size
            .max(self.minimum)
            .checked_next_multiple_of(requirements.alignment.max(65536))
            .ok_or_else(|| GpuError::Contract("Memory block size overflow".into()))?;
        let mut flags =
            vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let mut info = vk::MemoryAllocateInfo::default()
            .allocation_size(size)
            .memory_type_index(key.memory_type);
        if key.address {
            info = info.push(&mut flags);
        }
        // SAFETY: the block uses a compatible queried memory type and required address flags.
        let memory = unsafe { device.allocate_memory(&info, None) }.map_err(anyhow::Error::from)?;
        self.device.get_or_insert_with(|| Box::new(device.clone()));
        let mut ranges = Ranges::new(size);
        let offset = ranges
            .take(requirements.size, requirements.alignment)
            .expect("new block fits its queried allocation");
        self.blocks.push(Block {
            id,
            memory,
            size,
            key,
            ranges,
        });
        Ok(Slice {
            id,
            memory,
            offset,
            size: requirements.size,
        })
    }
    /// Unknown IDs are unpublished worker allocations, owned by their disposal batch.
    pub fn release(&mut self, slice: Slice) {
        if let Some(block) = self.blocks.iter_mut().find(|b| b.id == slice.id) {
            block.ranges.release(slice.offset, slice.size);
        }
    }
    pub fn append(&mut self, other: &mut Self) {
        if self.device.is_none() {
            self.device = other.device.take();
        }
        self.blocks.append(&mut other.blocks);
    }
    /// Empty blocks above a 64 MiB cache budget go to the native disposal worker.
    pub fn trim(&mut self) -> Vec<vk::DeviceMemory> {
        let mut idle = 0;
        let mut freed = Vec::new();
        let mut i = 0;
        while i < self.blocks.len() {
            let b = &self.blocks[i];
            if b.ranges.used != 0 || idle + b.size <= 64 * 1024 * 1024 {
                if b.ranges.used == 0 {
                    idle += b.size;
                }
                i += 1;
            } else {
                freed.push(self.blocks.swap_remove(i).memory);
            }
        }
        freed
    }
    pub fn drain(&mut self) -> impl Iterator<Item = vk::DeviceMemory> + '_ {
        self.blocks.drain(..).map(|b| b.memory)
    }
}
impl Drop for MemoryBlocks {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            for block in self.blocks.drain(..) {
                // SAFETY: the owning context/raw batch destroys all bound resources before its pool.
                unsafe {
                    device.free_memory(block.memory, None);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligned_ranges_split_coalesce_and_reuse_without_overlap() {
        let mut ranges = Ranges::new(1024);
        let a = ranges.take(100, 64).unwrap();
        let b = ranges.take(200, 256).unwrap();
        let c = ranges.take(128, 128).unwrap();
        assert_eq!(a % 64, 0);
        assert_eq!(b % 256, 0);
        assert_eq!(c % 128, 0);
        let mut live = [(a, 100), (b, 200), (c, 128)];
        live.sort();
        assert!(live.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0));
        for (at, size) in live {
            ranges.release(at, size);
        }
        assert_eq!(ranges.free, [(0, 1024)]);
        assert_eq!(ranges.used, 0);
        assert_eq!(ranges.take(1024, 1), Some(0));
        assert_eq!(ranges.take(1, 1), None);
    }
    #[test]
    fn late_replies_cannot_alias_a_reused_native_memory_handle() {
        use ash::vk::Handle;
        let memory = vk::DeviceMemory::from_raw(7);
        let mut ranges = Ranges::new(1024);
        ranges.take(128, 64).unwrap();
        let mut blocks = MemoryBlocks::default();
        blocks.blocks.push(Block {
            id: 2,
            memory,
            size: 1024,
            key: Key {
                memory_type: 0,
                address: false,
                class: Class::Buffer,
            },
            ranges,
        });
        blocks.release(Slice {
            id: 1,
            memory,
            offset: 0,
            size: 128,
        });
        assert_eq!(blocks.blocks[0].ranges.used, 128);
        blocks.release(Slice {
            id: 2,
            memory,
            offset: 0,
            size: 128,
        });
        assert_eq!(blocks.blocks[0].ranges.used, 0);
    }
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Vulkan allocation verification runs on Windows"
    )]
    fn native_private_allocations_share_blocks_without_overlapping() {
        use crate::{Device, Resource, vk, vulkan::VulkanDevice};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-gpu/suballocation");
        let mut gpu = VulkanDevice::new_raster(&[], root).unwrap();
        let usage = vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST;
        let a = gpu.create_buffer(4096, usage, false).unwrap();
        let b = gpu.create_buffer(4096, usage, false).unwrap();
        let ra = gpu.buffers.get(a).unwrap().raw.suballocation.unwrap();
        let rb = gpu.buffers.get(b).unwrap().raw.suballocation.unwrap();
        assert_eq!(ra.id, rb.id);
        assert_eq!(ra.memory, rb.memory);
        assert!(ra.offset + ra.size <= rb.offset || rb.offset + rb.size <= ra.offset);
        let image_usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
        let x = gpu
            .create_image(16, 16, vk::Format::R8G8B8A8_UNORM, image_usage)
            .unwrap();
        let y = gpu
            .create_image(16, 16, vk::Format::R8G8B8A8_UNORM, image_usage)
            .unwrap();
        let rx = gpu.images.get(x).unwrap().raw.suballocation.unwrap();
        let ry = gpu.images.get(y).unwrap().raw.suballocation.unwrap();
        assert_eq!(rx.id, ry.id);
        assert_ne!(rx.id, ra.id);
        assert!(rx.offset + rx.size <= ry.offset || ry.offset + ry.size <= rx.offset);
        for resource in [
            Resource::Buffer(a),
            Resource::Buffer(b),
            Resource::Image(x),
            Resource::Image(y),
        ] {
            gpu.retire(resource).unwrap();
        }
        gpu.collect().unwrap();
        gpu.assert_clean().unwrap();
    }
}
