#![allow(clippy::unwrap_used)]
//! Cube-array views on Vulkan: two cubes over twelve layers of a square image, each
//! face sampled by direction and stored through a 2D-array storage view.
use ara_gpu::{
    Access, Barrier, Device, GpuError, Queue, Resource, RootConstants,
    cooked::CookedPipeline,
    descriptors::{Format, ImageDesc},
    sampler::{Filter, SamplerDesc},
    texture::{ImageShape, ImageViewDesc, TextureDesc, ViewDimension},
    vk,
    vulkan::{Command, VulkanDevice},
};
use std::path::Path;

fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
fn layered(gpu: &mut VulkanDevice, size: [u32; 2], storage: bool) -> ara_gpu::Image {
    gpu.create_texture(&TextureDesc {
        image: ImageDesc {
            size,
            format: Format::Rgba32Float,
            sampled: true,
            storage,
            attachment: false,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape {
            layers: 12,
            ..Default::default()
        },
    })
    .unwrap()
}
fn contract(result: ara_gpu::Result<ara_gpu::HeapIndex>) -> bool {
    matches!(result, Err(GpuError::Contract(_)))
}

/// Layer l of a 4x4 image holds l + 1. A cube-array view over all twelve layers samples
/// face `i % 6` of cube `i / 6` along that face's axis, and a compute stage stores the
/// twelve results in the layers of a second image through its 2D-array storage view;
/// reading that image back gives 1 to 12 in order. A storage cube view, a view of a
/// non-square image and a view that is not whole cubes are contract errors.
///
/// Where the device does not report `imageCubeArray`, the view and the pipeline (its
/// module has an arrayed cube image type) must both be refused with a contract error,
/// and nothing else can be checked: the test says so and stops.
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn cube_array_views_sample_faces_by_direction_and_cubes_by_index() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu =
        VulkanDevice::new_raster(&[], root.join("../../target/ara-gpu/cube-arrays")).unwrap();
    let cooked = root.join("tests/cube_cooked");
    let metadata: CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("cubes.json")).unwrap()).unwrap();
    let source = layered(&mut gpu, [4, 4], false);
    let cubes = ImageViewDesc {
        dimension: ViewDimension::CubeArray,
        base_mip: 0,
        mip_count: 1,
        base_layer: 0,
        layer_count: 12,
    };
    if !gpu.context().info.cube_arrays {
        assert!(contract(gpu.sampled_view(source, cubes)));
        assert!(matches!(
            gpu.create_pipeline(&metadata, &cooked),
            Err(GpuError::Contract(_))
        ));
        eprintln!(
            "SKIPPED: this device does not report imageCubeArray; only the refusals of the \
             cube-array view and of the cube-array pipeline were checked"
        );
        return;
    }
    let slot = gpu.sampled_view(source, cubes).unwrap();
    assert_eq!(gpu.sampled_view(source, cubes).unwrap(), slot);
    let stored = layered(&mut gpu, [4, 4], true);
    assert!(contract(gpu.storage_view(stored, cubes)));
    assert!(contract(gpu.sampled_view(
        source,
        ImageViewDesc {
            layer_count: 8,
            ..cubes
        }
    )));
    let oblong = layered(&mut gpu, [8, 4], false);
    assert!(contract(gpu.sampled_view(oblong, cubes)));

    let pipeline = gpu.create_pipeline(&metadata, &cooked).unwrap();
    let target = gpu
        .create_texture(&TextureDesc {
            image: ImageDesc {
                size: [1, 1],
                format: Format::Rgba32Float,
                sampled: false,
                storage: true,
                attachment: false,
                copy_source: true,
                copy_destination: false,
            },
            shape: ImageShape {
                layers: 12,
                ..Default::default()
            },
        })
        .unwrap();
    let layers = gpu
        .storage_view(
            target,
            ImageViewDesc {
                dimension: ViewDimension::D2Array,
                ..cubes
            },
        )
        .unwrap();
    let sampler = gpu
        .sampler(&SamplerDesc {
            min: Filter::Nearest,
            mag: Filter::Nearest,
            mip: Filter::Nearest,
            ..SamplerDesc::default()
        })
        .unwrap();
    // Every texel of layer l is (l + 1, 0, 0, 1).
    let texels: Vec<f32> = (0..12)
        .flat_map(|layer| [layer as f32 + 1., 0., 0., 1.].repeat(16))
        .collect();
    let bytes: &[u8] = bytemuck::cast_slice(&texels);
    let staging = gpu
        .create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .unwrap();
    gpu.upload(staging, bytes).unwrap();
    let output = gpu.create_readback_buffer(12 * 16).unwrap();
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers([
        edge(
            Resource::Buffer(staging),
            Access::HostWrite,
            Access::CopySource,
        ),
        edge(
            Resource::Image(source),
            Access::Undefined,
            Access::CopyDestination,
        ),
        edge(Resource::Image(target), Access::Undefined, Access::Storage),
    ]);
    rec.push(Command::UploadImage {
        source: staging,
        destination: source,
    });
    rec.barriers([edge(
        Resource::Image(source),
        Access::CopyDestination,
        Access::ShaderRead,
    )]);
    rec.push(Command::Pipeline(pipeline));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, layers.index, sampler.index, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([1, 1, 12]));
    rec.barriers([edge(
        Resource::Image(target),
        Access::Storage,
        Access::CopySource,
    )]);
    rec.push(Command::ReadImage {
        source: target,
        destination: output,
    });
    rec.barriers([edge(
        Resource::Buffer(output),
        Access::CopyDestination,
        Access::HostRead,
    )]);
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    let read: Vec<f32> = bytemuck::cast_slice::<u8, f32>(&gpu.read(output).unwrap())
        .as_chunks::<4>()
        .0
        .iter()
        .map(|texel| texel[0])
        .collect();
    assert_eq!(read, (1..=12).map(|v| v as f32).collect::<Vec<_>>());
    for image in [source, stored, oblong, target] {
        Device::retire(&mut gpu, Resource::Image(image)).unwrap();
    }
    Device::retire_pipeline(&mut gpu, pipeline).unwrap();
    Device::collect(&mut gpu).unwrap();
}
