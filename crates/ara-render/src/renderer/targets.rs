//! View target descriptions shared by immediate and worker allocation.
use super::*;
use ara_gpu::vulkan::{
    BufferPreparation, ImagePreparation, PreparedResources, ResourcePreparation,
};

pub(super) fn request(render: [u32; 2], display: [u32; 2]) -> ResourcePreparation {
    let images = (0..TARGET_COUNT)
        .map(|target| {
            let format = match target {
                ALBEDO => vk::Format::R8G8B8A8_SRGB,
                NORMAL => vk::Format::R16G16_UNORM,
                MATERIAL => vk::Format::R8G8B8A8_UNORM,
                EMISSIVE => vk::Format::B10G11R11_UFLOAT_PACK32,
                ID => vk::Format::R32_SFLOAT,
                DEPTH => vk::Format::D32_SFLOAT,
                OUTPUT => vk::Format::R8G8B8A8_UNORM,
                MOTION => vk::Format::R16G16_SFLOAT,
                WIRE => vk::Format::R8_UNORM,
                // HDR is pre-exposed (`HdrExposure`), so half floats keep its range.
                HDR | HISTORY0 | HISTORY1 => vk::Format::R16G16B16A16_SFLOAT,
                _ => vk::Format::R32G32B32A32_SFLOAT,
            };
            ImagePreparation {
                size: if (HISTORY0..=OUTPUT).contains(&target) {
                    display
                } else {
                    render
                },
                format,
                mip_levels: 1,
                usage: vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | if target == DEPTH {
                        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                    } else if (HISTORY0..=OUTPUT).contains(&target) {
                        vk::ImageUsageFlags::STORAGE
                    } else {
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                    },
            }
        })
        .collect();
    let bytes = |[w, h]: [u32; 2]| u64::from(w) * u64::from(h) * 4;
    let buffers = [
        (
            u64::from(render[0].div_ceil(16))
                * u64::from(render[1].div_ceil(16))
                * (crate::scene::TILE_LIGHTS as u64 + 1)
                * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            false,
        ),
        (bytes(display), vk::BufferUsageFlags::TRANSFER_DST, true),
        (bytes(render), vk::BufferUsageFlags::TRANSFER_DST, true),
    ]
    .into_iter()
    .map(|(size, usage, host)| BufferPreparation {
        size,
        usage,
        host,
        data: Default::default(),
    })
    .collect();
    ResourcePreparation { images, buffers }
}

/// Synchronous constructors/captures use the same descriptions as a prepared view.
pub(super) fn allocate(
    gpu: &mut GpuDevice,
    request: ResourcePreparation,
) -> Result<PreparedResources> {
    let mut resources = PreparedResources {
        images: Vec::new(),
        buffers: Vec::new(),
    };
    let result = (|| {
        for b in request.buffers {
            resources
                .buffers
                .push(gpu.create_buffer(b.size, b.usage, b.host)?);
        }
        for i in request.images {
            resources
                .images
                .push(gpu.create_mip_image(i.size, i.format, i.mip_levels, i.usage)?);
        }
        Ok(())
    })();
    if let Err(error) = result {
        retire(gpu, resources)?;
        return Err(error);
    }
    Ok(resources)
}

pub(super) fn retire(gpu: &mut GpuDevice, resources: PreparedResources) -> Result<()> {
    for buffer in resources.buffers {
        gpu.retire(Resource::Buffer(buffer))?;
    }
    for image in resources.images {
        gpu.retire(Resource::Image(image))?;
    }
    // Rejected batches were never submitted. Release their descriptor slots now,
    // so retries under heap pressure cannot accumulate unpublished bindings.
    gpu.collect()?;
    Ok(())
}

/// Bind adopted resources; the caller retires the whole batch on failure.
pub(super) fn bind(gpu: &mut GpuDevice, resources: PreparedResources) -> Result<Targets> {
    ensure!(
        resources.images.len() == TARGET_COUNT && resources.buffers.len() == 3,
        "Invalid target batch"
    );
    let images = resources.images;
    let sampled = images
        .iter()
        .enumerate()
        .map(|(i, &image)| Ok(gpu.sampled_image(image, i == DEPTH)?.index))
        .collect::<Result<_>>()?;
    let storage = [
        gpu.storage_image(images[HISTORY0])?.index,
        gpu.storage_image(images[HISTORY1])?.index,
        gpu.storage_image(images[OUTPUT])?.index,
    ];
    let [tiles, read_color, read_ids] = resources.buffers.as_slice() else {
        unreachable!("target buffer count checked")
    };
    Ok(Targets {
        tile_index: gpu.storage_buffer(*tiles)?.index,
        tiles: *tiles,
        read_color: *read_color,
        read_ids: *read_ids,
        images,
        sampled,
        storage,
        states: vec![
            State {
                access: Access::Undefined,
                queue: Queue::Graphics
            };
            TARGET_COUNT
        ],
        history: None,
    })
}

impl Renderer {
    pub(super) fn targets(
        gpu: &mut GpuDevice,
        render: [u32; 2],
        display: [u32; 2],
    ) -> Result<Targets> {
        let resources = allocate(gpu, request(render, display))?;
        let owned = PreparedResources {
            images: resources.images.to_vec(),
            buffers: resources.buffers.to_vec(),
        };
        match bind(gpu, resources) {
            Ok(targets) => Ok(targets),
            Err(error) => {
                retire(gpu, owned)?;
                Err(error)
            }
        }
    }
}
