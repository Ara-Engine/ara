//! Native volume, array, mip-region and view behavior.
use super::*;
use crate::commands::Command;
use crate::descriptors::{Format, ImageDesc};
use crate::texture::*;

fn texture(gpu: &mut MetalDevice, shape: ImageShape) -> Image {
    gpu.create_texture(&TextureDesc {
        image: ImageDesc {
            size: [8, 8],
            format: Format::R32Uint,
            sampled: true,
            storage: true,
            attachment: false,
            copy_source: true,
            copy_destination: true,
        },
        shape,
    })
    .unwrap()
}
fn barrier(image: Image, before: Access, after: Access) -> Barrier {
    Barrier {
        resource: Resource::Image(image),
        before,
        after,
        handoff: None,
    }
}
#[test]
fn volumes_and_arrays_roundtrip_mips_and_checked_regions() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    for shape in [
        ImageShape {
            dimension: TextureDimension::D3,
            depth: 8,
            mip_levels: 4,
            ..Default::default()
        },
        ImageShape {
            layers: 4,
            mip_levels: 4,
            ..Default::default()
        },
    ] {
        let image = texture(&mut gpu, shape);
        let words: usize = (0..shape.mip_levels)
            .map(|m| shape.extent([8, 8], m).into_iter().product::<u32>() * shape.layers)
            .sum::<u32>() as usize;
        let data: Vec<u32> = (0..words as u32).collect();
        let upload = gpu
            .create_buffer((words * 4) as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)
            .unwrap();
        let readback = gpu.create_readback_buffer((words * 4) as u64).unwrap();
        gpu.upload(upload, bytemuck::cast_slice(&data)).unwrap();
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::UploadImage {
            source: upload,
            destination: image,
        });
        rec.barriers([barrier(image, Access::CopyDestination, Access::CopySource)]);
        rec.push(Command::ReadImage {
            source: image,
            destination: readback,
        });
        let p = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(p).unwrap();
        assert_eq!(
            gpu.read(readback).unwrap(),
            bytemuck::cast_slice::<u32, u8>(&data)
        );
        let region = ImageRegion {
            mip: 1,
            origin: [1, 1, if shape.depth > 1 { 1 } else { 0 }],
            extent: [2, 2, if shape.depth > 1 { 2 } else { 1 }],
            base_layer: if shape.layers > 1 { 1 } else { 0 },
            layer_count: if shape.layers > 1 { 2 } else { 1 },
        };
        let copy = texture(&mut gpu, shape);
        let (_, bytes) = shape.region([8, 8], region, 1, 4).unwrap();
        let source = gpu
            .create_buffer(bytes + 8, vk::BufferUsageFlags::TRANSFER_SRC, true)
            .unwrap();
        let out = gpu.create_readback_buffer(bytes + 8).unwrap();
        gpu.upload(source, &vec![0x6d; bytes as usize + 8]).unwrap();
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::FillBuffer {
            destination: out,
            offset: 0,
            size: bytes + 8,
            value: 0,
        });
        rec.push(Command::UploadImageRegion {
            source,
            destination: image,
            buffer_offset: 4,
            region,
        });
        rec.barriers([barrier(image, Access::CopyDestination, Access::CopySource)]);
        rec.push(Command::CopyImageRegion {
            source: image,
            destination: copy,
            from: region,
            to: region,
        });
        rec.barriers([
            barrier(copy, Access::CopyDestination, Access::CopySource),
            Barrier {
                resource: Resource::Buffer(out),
                before: Access::CopyDestination,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        rec.push(Command::ReadImageRegion {
            source: copy,
            destination: out,
            buffer_offset: 4,
            region,
        });
        let p = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(p).unwrap();
        let data = gpu.read(out).unwrap();
        assert_eq!(&data[..4], &[0; 4]);
        assert_eq!(&data[data.len() - 4..], &[0; 4]);
        assert!(data[4..data.len() - 4].iter().all(|b| *b == 0x6d));
        let invalid = ImageRegion {
            mip: shape.mip_levels,
            ..region
        };
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::UploadImageRegion {
            source,
            destination: image,
            buffer_offset: 0,
            region: invalid,
        });
        assert!(gpu.submit(rec.finish(), &[]).is_err());
    }
}
#[test]
fn array_mip_views_are_shader_visible_and_retire_with_the_parent() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let image = texture(
        &mut gpu,
        ImageShape {
            layers: 4,
            mip_levels: 4,
            ..Default::default()
        },
    );
    let desc = ImageViewDesc {
        dimension: ViewDimension::D2Array,
        base_mip: 1,
        mip_count: 1,
        base_layer: 1,
        layer_count: 2,
    };
    let slot = gpu.sampled_view(image, desc).unwrap();
    assert_eq!(gpu.sampled_view(image, desc).unwrap(), slot);
    let store = gpu.storage_view(image, desc).unwrap();
    let input = gpu
        .create_buffer(128, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .unwrap();
    let words: Vec<u32> = (100..132).collect();
    gpu.upload(input, bytemuck::cast_slice(&words)).unwrap();
    let output = gpu
        .create_buffer(8, vk::BufferUsageFlags::STORAGE_BUFFER, true)
        .unwrap();
    let out = gpu.storage_buffer(output).unwrap();
    let shader = r#"
struct ArrayEntry { ulong unused; texture2d_array<uint> image; ulong metadata; };
kernel void arrayProbe(constant TopLevel& t [[buffer(2)]]) {
    device ArrayEntry* a = (device ArrayEntry*)t.textures;
    t.buffers[t.root[1]].data[0] = a[t.root[0]].image.read(uint2(0,0),0).x;
    t.buffers[t.root[1]].data[1] = a[t.root[0]].image.read(uint2(0,0),1).x;
}
"#;
    let p = gpu.msl_pipeline_from(
        shader,
        &[(cooked::Stage::Compute, "arrayProbe")],
        &cooked::RenderState::default(),
        Some([1, 1, 1]),
    );
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::UploadImageRegion {
        source: input,
        destination: image,
        buffer_offset: 0,
        region: ImageRegion {
            mip: 1,
            origin: [0; 3],
            extent: [4, 4, 1],
            base_layer: 1,
            layer_count: 2,
        },
    });
    rec.barriers([barrier(image, Access::CopyDestination, Access::ShaderRead)]);
    rec.push(Command::Pipeline(p));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, out.index, 0, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([1, 1, 1]));
    let p = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(p).unwrap();
    assert_eq!(
        bytemuck::cast_slice::<u8, u32>(&gpu.read(output).unwrap()),
        &[100, 116]
    );
    assert!(store.index < abi::STORAGE_IMAGES);
    Device::retire(&mut gpu, Resource::Image(image)).unwrap();
    Device::collect(&mut gpu).unwrap();
    assert_eq!(gpu.descriptor_usage().sampled_images, 0);
    assert_eq!(gpu.descriptor_usage().storage_images, 0);
}

/// Two cubes over twelve layers of a square image: a cube-array view samples each face
/// by direction and each cube by index, sharing the layers a 2D-array view names, and
/// retires with its parent. A storage cube view, a non-square image and a layer count
/// that is not whole cubes are refused.
#[test]
fn cube_array_views_sample_faces_by_direction_and_cubes_by_index() {
    use crate::sampler::SamplerDesc;
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    assert!(gpu.info().cube_arrays);
    let create = |gpu: &mut MetalDevice, size: [u32; 2]| {
        gpu.create_texture(&TextureDesc {
            image: ImageDesc {
                size,
                format: Format::R32Float,
                sampled: true,
                storage: true,
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
    };
    let image = create(&mut gpu, [4, 4]);
    let cubes = ImageViewDesc {
        dimension: ViewDimension::CubeArray,
        base_mip: 0,
        mip_count: 1,
        base_layer: 0,
        layer_count: 12,
    };
    let slot = gpu.sampled_view(image, cubes).unwrap();
    assert_eq!(gpu.sampled_view(image, cubes).unwrap(), slot);
    assert!(gpu.storage_view(image, cubes).is_err());
    assert!(
        gpu.sampled_view(
            image,
            ImageViewDesc {
                layer_count: 8,
                ..cubes
            }
        )
        .is_err()
    );
    let oblong = create(&mut gpu, [8, 4]);
    assert!(gpu.sampled_view(oblong, cubes).is_err());
    // Layer l holds l + 1 in every texel.
    let texels: Vec<f32> = (0..12).flat_map(|layer| [layer as f32 + 1.; 16]).collect();
    let input = gpu
        .create_buffer(4 * 16 * 12, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .unwrap();
    gpu.upload(input, bytemuck::cast_slice(&texels)).unwrap();
    let output = gpu
        .create_buffer(4 * 12, vk::BufferUsageFlags::STORAGE_BUFFER, true)
        .unwrap();
    let out = gpu.storage_buffer(output).unwrap();
    let sampler = gpu.sampler(&SamplerDesc::default()).unwrap();
    let shader = r#"
struct CubeEntry { ulong unused; texturecube_array<float> image; ulong metadata; };
kernel void cubeProbe(constant TopLevel& t [[buffer(2)]]) {
    device CubeEntry* c = (device CubeEntry*)t.textures;
    const float3 toward[6] = { float3(1,0,0), float3(-1,0,0), float3(0,1,0), float3(0,-1,0), float3(0,0,1), float3(0,0,-1) };
    for (uint i = 0; i < 12; i++) {
        float v = c[t.root[0]].image.sample(t.samplers[t.root[2]].state, toward[i % 6], i / 6, level(0)).x;
        t.buffers[t.root[1]].data[i] = as_type<uint>(v);
    }
}
"#;
    let p = gpu.msl_pipeline_from(
        shader,
        &[(cooked::Stage::Compute, "cubeProbe")],
        &cooked::RenderState::default(),
        Some([1, 1, 1]),
    );
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::UploadImageRegion {
        source: input,
        destination: image,
        buffer_offset: 0,
        region: ImageRegion {
            mip: 0,
            origin: [0; 3],
            extent: [4, 4, 1],
            base_layer: 0,
            layer_count: 12,
        },
    });
    rec.barriers([barrier(image, Access::CopyDestination, Access::ShaderRead)]);
    rec.push(Command::Pipeline(p));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, out.index, sampler.index, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([1, 1, 1]));
    let p = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(p).unwrap();
    let read: Vec<f32> = bytemuck::cast_slice::<u8, u32>(&gpu.read(output).unwrap())
        .iter()
        .map(|w| f32::from_bits(*w))
        .collect();
    assert_eq!(read, (1..=12).map(|v| v as f32).collect::<Vec<_>>());
    for image in [image, oblong] {
        Device::retire(&mut gpu, Resource::Image(image)).unwrap();
    }
    Device::collect(&mut gpu).unwrap();
    assert_eq!(gpu.descriptor_usage().sampled_images, 0);
}
