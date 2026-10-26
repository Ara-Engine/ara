use super::*;
use anyhow::ensure;

pub(crate) fn upload(
    device: &ash::Device,
    buffer: BufferAllocation,
    offset: u64,
    data: &[u8],
) -> Result<()> {
    ensure!(buffer.host, "Buffer is not host visible");
    ensure!(
        offset
            .checked_add(data.len() as u64)
            .is_some_and(|end| end <= buffer.size),
        "Upload out of bounds"
    );
    // SAFETY: Caller uploads only before submission/after completion. The
    // host-coherent mapped allocation has enough capacity and is unmapped here.
    unsafe {
        let ptr = device.map_memory(buffer.memory, 0, buffer.size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(
            data.as_ptr(),
            ptr.cast::<u8>().add(offset as usize),
            data.len(),
        );
        device.unmap_memory(buffer.memory);
    }
    Ok(())
}
