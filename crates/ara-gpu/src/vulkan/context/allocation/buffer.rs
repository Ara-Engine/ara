use super::super::buffer_memory::{BufferMemory, MemoryKey};
use super::*;
use ash::vk::TaggedStructure;

pub(crate) struct BufferRequest {
    pub size: u64,
    pub usage: vk::BufferUsageFlags,
    pub host: bool,
    pub preferred: vk::MemoryPropertyFlags,
}
pub(crate) fn pooled_buffer(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    pool: &mut BufferMemory,
    mut blocks: Option<&mut super::super::memory_blocks::MemoryBlocks>,
    request: BufferRequest,
) -> Result<BufferAllocation> {
    let BufferRequest {
        size,
        usage,
        host,
        preferred,
    } = request;
    let mut suballocation = None;
    // SAFETY: Allocations satisfy queried requirements; resources are retained
    // until device idle in Drop. Exclusive ownership is managed in render.rs.
    unsafe {
        let handle = device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(size)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?;
        let mut memory = vk::DeviceMemory::null();
        let result = (|| -> Result<_> {
            let mut dedicated = vk::MemoryDedicatedRequirements::default();
            let mut requirements = vk::MemoryRequirements2::default().push(&mut dedicated);
            device.get_buffer_memory_requirements2(
                &vk::BufferMemoryRequirementsInfo2::default().buffer(handle),
                &mut requirements,
            );
            let requirements = requirements.memory_requirements;
            let flags = if host {
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
            } else {
                vk::MemoryPropertyFlags::DEVICE_LOCAL
            };
            let memory_type_index = memory_type(
                memory_properties,
                requirements.memory_type_bits,
                flags | preferred,
            )
            .or_else(|_| memory_type(memory_properties, requirements.memory_type_bits, flags))?;
            let key = MemoryKey {
                size: requirements.size,
                memory_type: memory_type_index,
                device_address: usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS),
            };
            let memory_key = (dedicated.requires_dedicated_allocation == vk::FALSE).then_some(key);
            let mut address_flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().buffer(handle);
            let mut allocation = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type_index);
            if key.device_address {
                allocation = allocation.push(&mut address_flags);
            }
            if memory_key.is_none() {
                allocation = allocation.push(&mut dedicated_info);
            }
            if !host
                && dedicated.requires_dedicated_allocation == vk::FALSE
                && let Some(blocks) = blocks.as_deref_mut()
            {
                let slice = blocks.allocate(
                    device,
                    super::super::memory_blocks::Key {
                        memory_type: memory_type_index,
                        address: key.device_address,
                        class: super::super::memory_blocks::Class::Buffer,
                    },
                    requirements,
                )?;
                memory = slice.memory;
                suballocation = Some(slice);
            } else {
                memory = match memory_key.and_then(|key| pool.take(key)) {
                    Some(memory) => memory,
                    None => device.allocate_memory(&allocation, None)?,
                };
            }
            device.bind_buffer_memory(handle, memory, suballocation.map_or(0, |s| s.offset))?;
            Ok(BufferAllocation {
                handle,
                memory,
                suballocation,
                size,
                host,
                usage,
                memory_key: if suballocation.is_some() {
                    None
                } else {
                    memory_key
                },
            })
        })();
        let buffer = match result {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(handle, None);
                if let Some(slice) = suballocation {
                    blocks
                        .expect("pooled allocation has an owner")
                        .release(slice);
                } else if memory != vk::DeviceMemory::null() {
                    device.free_memory(memory, None);
                }
                return Err(error);
            }
        };
        Ok(buffer)
    }
}
