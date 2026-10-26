use super::super::buffer_memory::{BufferMemory, MemoryKey};
use super::*;
use ash::vk::TaggedStructure;

pub(in crate::vulkan::context) fn buffer(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    pool: &mut BufferMemory,
    size: u64,
    usage: vk::BufferUsageFlags,
    host: bool,
    preferred: vk::MemoryPropertyFlags,
) -> Result<BufferAllocation> {
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
            memory = match memory_key.and_then(|key| pool.take(key)) {
                Some(memory) => memory,
                None => device.allocate_memory(&allocation, None)?,
            };
            device.bind_buffer_memory(handle, memory, 0)?;
            Ok(BufferAllocation {
                handle,
                memory,
                size,
                host,
                usage,
                memory_key,
            })
        })();
        let buffer = match result {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(handle, None);
                if memory != vk::DeviceMemory::null() {
                    device.free_memory(memory, None);
                }
                return Err(error);
            }
        };
        Ok(buffer)
    }
}
