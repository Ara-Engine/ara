//! Idle buffer memory, admitted only after all referencing queue timelines retire.
use ash::vk;

/// Exact allocation compatibility. Every fresh buffer binds at aligned offset zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MemoryKey {
    pub size: u64,
    pub memory_type: u32,
    pub device_address: bool,
}

/// Bounds retained VRAM and metadata even when a workload changes sizes every frame.
#[derive(Default)]
pub(super) struct BufferMemory {
    idle: Vec<(MemoryKey, vk::DeviceMemory)>,
    bytes: u64,
}

impl BufferMemory {
    const MAX_BYTES: u64 = 64 * 1024 * 1024;
    const MAX_BLOCKS: usize = 256;

    pub fn take(&mut self, key: MemoryKey) -> Option<vk::DeviceMemory> {
        let index = self.idle.iter().position(|(cached, _)| *cached == key)?;
        let (_, memory) = self.idle.swap_remove(index);
        self.bytes -= key.size;
        Some(memory)
    }

    /// Returns false when the caller must free this block instead of retaining it.
    pub fn retain(&mut self, key: MemoryKey, memory: vk::DeviceMemory) -> bool {
        if key.size > Self::MAX_BYTES - self.bytes || self.idle.len() == Self::MAX_BLOCKS {
            return false;
        }
        self.idle.push((key, memory));
        self.bytes += key.size;
        true
    }

    pub fn drain(&mut self) -> impl Iterator<Item = vk::DeviceMemory> + '_ {
        self.bytes = 0;
        self.idle.drain(..).map(|(_, memory)| memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ash::vk::Handle;

    #[test]
    fn reuse_requires_the_same_allocation_properties_and_transfers_ownership() {
        let mut pool = BufferMemory::default();
        let key = MemoryKey {
            size: 4096,
            memory_type: 2,
            device_address: true,
        };
        let memory = vk::DeviceMemory::from_raw(7);
        assert!(pool.retain(key, memory));
        assert!(pool.take(MemoryKey { size: 8192, ..key }).is_none());
        assert!(
            pool.take(MemoryKey {
                memory_type: 3,
                ..key
            })
            .is_none()
        );
        assert!(
            pool.take(MemoryKey {
                device_address: false,
                ..key
            })
            .is_none()
        );
        assert_eq!(pool.take(key), Some(memory));
        assert!(pool.take(key).is_none());
        assert_eq!(pool.bytes, 0);
        assert!(pool.retain(key, memory));
        assert_eq!(pool.drain().collect::<Vec<_>>(), vec![memory]);
        assert!(pool.drain().next().is_none());
    }

    #[test]
    fn retention_is_bounded_by_both_bytes_and_block_count() {
        let mut pool = BufferMemory::default();
        let memory = vk::DeviceMemory::from_raw(7);
        let full = MemoryKey {
            size: BufferMemory::MAX_BYTES,
            memory_type: 0,
            device_address: false,
        };
        assert!(pool.retain(full, memory));
        assert!(!pool.retain(full, memory));
        assert_eq!(pool.take(full), Some(memory));
        let small = MemoryKey { size: 1, ..full };
        for _ in 0..BufferMemory::MAX_BLOCKS {
            assert!(pool.retain(small, memory));
        }
        assert!(!pool.retain(small, memory));
        assert_eq!(pool.drain().count(), BufferMemory::MAX_BLOCKS);
    }
}
