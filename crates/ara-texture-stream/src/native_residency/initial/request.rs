use super::*;
use ara_gpu::{
    GpuError, vk,
    vulkan::{BufferPreparation, ImagePreparation, InitialData, ResourcePreparation},
};

pub(super) fn build(
    entries: &[Entry],
    plan: &[u32],
    selected: &[usize],
) -> Result<ResourcePreparation> {
    let mut request = ResourcePreparation::default();
    for &index in selected {
        let entry = &entries[index];
        let first = plan[index];
        let mip = entry
            .source
            .mip(first)
            .ok_or_else(|| anyhow::anyhow!("native mip plan"))?;
        let (format, _) = upload::format(entry)?;
        request.images.push(ImagePreparation {
            size: mip.size,
            format,
            mip_levels: entry.source.mip_count() - first,
            usage: vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC,
        });
        let source = Arc::clone(&entry.source);
        request.buffers.push(BufferPreparation {
            size: plan::bytes(entry, first)?,
            usage: vk::BufferUsageFlags::TRANSFER_SRC,
            host: true,
            data: InitialData::deferred(move || {
                source
                    .stage(first)
                    .map_err(|error| GpuError::Contract(error.to_string()))
            }),
        });
    }
    request.buffers.push(BufferPreparation {
        size: (entries.len() * std::mem::size_of::<Row>()) as u64,
        usage: vk::BufferUsageFlags::STORAGE_BUFFER,
        host: true,
        data: InitialData::default(),
    });
    Ok(request)
}
