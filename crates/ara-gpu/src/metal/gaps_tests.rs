//! Portable GPU gap tests that execute native Metal commands without new shaders.
use super::*;
use crate::commands::Command;

#[test]
fn fill_and_large_ordered_updates_preserve_ranges_and_in_flight_data() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let size = 1024 * 1024;
    let target = gpu
        .create_buffer(
            size,
            vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::TRANSFER_SRC,
            false,
        )
        .unwrap();
    let readback = gpu.create_readback_buffer(size).unwrap();
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::FillBuffer {
        destination: target,
        offset: 0,
        size,
        value: 0x12345678,
    });
    rec.barriers([Barrier {
        resource: Resource::Buffer(target),
        before: Access::CopyDestination,
        after: Access::CopySource,
        handoff: None,
    }]);
    rec.push(Command::CopyBuffer {
        source: target,
        destination: readback,
        size,
    });
    let first = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(first).unwrap();
    assert!(
        gpu.read(readback)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|word| *word == 0x12345678u32.to_le_bytes())
    );
    // Hold the queue so the second payload is recorded while the first remains in use.
    let stall = gpu.device.newSharedEvent().unwrap();
    gpu.wait_event(&stall, 1);
    gpu.begin_deferred_completion().unwrap();
    for value in [17, 29] {
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::UpdateBuffer {
            destination: target,
            offset: 4,
            bytes: vec![value; size as usize - 8],
        });
        rec.push(Command::CopyBuffer {
            source: target,
            destination: readback,
            size,
        });
        gpu.submit(rec.finish(), &[]).unwrap();
    }
    let mut frame = gpu.end_deferred_completion().unwrap();
    stall.setSignaledValue(1);
    gpu.wait_completion(&mut frame).unwrap();
    let bytes = gpu.read(readback).unwrap();
    assert_eq!(&bytes[..4], &0x12345678u32.to_le_bytes());
    assert!(bytes[4..bytes.len() - 4].iter().all(|b| *b == 29));
    assert_eq!(&bytes[bytes.len() - 4..], &0x12345678u32.to_le_bytes());
    for (offset, length) in [(1, 4), (0, 0), (0, 3), (size, 4), (u64::MAX - 3, 8)] {
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::FillBuffer {
            destination: target,
            offset,
            size: length,
            value: 0,
        });
        assert!(gpu.submit(rec.finish(), &[]).is_err());
    }
}

#[test]
fn immutable_samplers_filter_address_compare_and_deduplicate() {
    use crate::cooked::{RenderState, Stage};
    use crate::sampler::{AddressMode as A, CompareOp, Filter as F, SamplerDesc};
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let texture = gpu
        .create_image(
            2,
            1,
            vk::Format::R32_SFLOAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )
        .unwrap();
    let slot = gpu.sampled_image(texture, false).unwrap();
    let upload = gpu
        .create_buffer(8, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .unwrap();
    gpu.upload(upload, bytemuck::cast_slice(&[0.25f32, 0.75]))
        .unwrap();
    let out = gpu
        .create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)
        .unwrap();
    let out_slot = gpu.storage_buffer(out).unwrap();
    let shader = r#"
kernel void probe(constant TopLevel& t [[buffer(2)]]) {
    float v = t.textures[t.root[0]].texture.sample(t.samplers[t.root[1]].state, float2(as_type<float>(t.root[3]),0.5)).x;
    t.buffers[t.root[2]].data[t.root[4]] = as_type<uint>(v);
}
"#;
    let p = gpu.msl_pipeline_from(
        shader,
        &[(Stage::Compute, "probe")],
        &RenderState::default(),
        Some([1, 1, 1]),
    );
    let cases = [
        (F::Nearest, A::ClampToEdge, 1.25, 0.75),
        (F::Nearest, A::Repeat, 1.25, 0.25),
        (F::Nearest, A::MirrorRepeat, 1.25, 0.75),
        (F::Linear, A::ClampToEdge, 0.5, 0.5),
    ];
    let mut rec = gpu.begin(Queue::Compute);
    rec.push(Command::UploadImage {
        source: upload,
        destination: texture,
    });
    rec.barriers([Barrier {
        resource: Resource::Image(texture),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    rec.push(Command::Pipeline(p));
    for (i, (filter, address, u, _)) in cases.iter().enumerate() {
        let desc = SamplerDesc {
            min: *filter,
            mag: *filter,
            mip: *filter,
            address: [*address; 3],
            ..SamplerDesc::default()
        };
        let sampler = gpu.sampler(&desc).unwrap();
        assert_eq!(sampler, gpu.sampler(&desc).unwrap());
        rec.push(Command::Constants(RootConstants {
            words: [
                slot.index,
                sampler.index,
                out_slot.index,
                (*u as f32).to_bits(),
                i as u32,
                0,
                0,
                0,
            ],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
    }
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    for (bytes, (_, _, _, expected)) in gpu.read(out).unwrap().as_chunks::<4>().0.iter().zip(cases)
    {
        assert!((f32::from_le_bytes(*bytes) - expected).abs() < 0.001);
    }
    for compare in [
        CompareOp::Less,
        CompareOp::LessEqual,
        CompareOp::Greater,
        CompareOp::GreaterEqual,
        CompareOp::Always,
    ] {
        gpu.sampler(&SamplerDesc {
            compare: Some(compare),
            ..SamplerDesc::default()
        })
        .unwrap();
    }
    gpu.sampler(&SamplerDesc {
        anisotropy: 16,
        ..SamplerDesc::default()
    })
    .unwrap();
    assert!(
        gpu.sampler(&SamplerDesc {
            min: F::Nearest,
            anisotropy: 16,
            ..SamplerDesc::default()
        })
        .is_err()
    );
}

#[test]
fn runtime_buffer_heap_addresses_slots_beyond_128_without_an_abi_change() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let mut last = None;
    for _ in 0..257 {
        let b = gpu
            .create_buffer(4, vk::BufferUsageFlags::STORAGE_BUFFER, true)
            .unwrap();
        let slot = gpu.storage_buffer(b).unwrap();
        last = Some((b, slot));
    }
    let (buffer, slot) = last.unwrap();
    assert_eq!(slot.index, 256);
    let p = gpu.msl_pipeline(
        &[(cooked::Stage::Compute, "fill")],
        &cooked::RenderState::default(),
        Some([1, 1, 1]),
    );
    let mut rec = gpu.begin(Queue::Compute);
    rec.push(Command::Pipeline(p));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, 0x12345678, 1, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([1, 1, 1]));
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    assert_eq!(gpu.read(buffer).unwrap(), 0x12345678u32.to_le_bytes());
}

#[test]
#[ignore = "CPU timing; run through ara-dev gpu"]
fn barrier_recording_timing() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    gpu.set_pass_timing(false);
    let b = gpu
        .create_buffer(4096, vk::BufferUsageFlags::TRANSFER_DST, false)
        .unwrap();
    for batched in [false, true] {
        let mut samples = Vec::new();
        for iteration in 0..120 {
            let mut rec = gpu.begin(Queue::Graphics);
            rec.push(Command::FillBuffer {
                destination: b,
                offset: 0,
                size: 4096,
                value: 0,
            });
            let barriers = (0..256).map(|_| Barrier {
                resource: Resource::Buffer(b),
                before: Access::CopyDestination,
                after: Access::CopyDestination,
                handoff: None,
            });
            if batched {
                rec.barriers(barriers);
            } else {
                for barrier in barriers {
                    rec.push(Command::Barrier(barrier));
                }
            }
            let list = rec.finish();
            let start = std::time::Instant::now();
            let p = gpu.submit(list, &[]).unwrap();
            let elapsed = start.elapsed().as_secs_f64() * 1e6;
            gpu.wait(p).unwrap();
            if iteration >= 20 {
                samples.push(elapsed);
            }
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "barriers256 mode={} median_cpu_us={:.3}",
            if batched { "recorder" } else { "individual" },
            samples[samples.len() / 2]
        );
    }
}

#[test]
fn comparison_samplers_read_the_bindless_depth_view() {
    use crate::sampler::{CompareOp, Filter, SamplerDesc};
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let depth = gpu
        .create_image(
            1,
            1,
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )
        .unwrap();
    let image = gpu.sampled_image(depth, true).unwrap();
    let input = gpu
        .create_buffer(4, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .unwrap();
    gpu.upload(input, bytemuck::cast_slice(&[0.25f32])).unwrap();
    let color = gpu
        .create_image(
            1,
            1,
            vk::Format::R32_SFLOAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )
        .unwrap();
    let regular_image = gpu.sampled_image(color, false).unwrap();
    let regular = gpu.sampler(&SamplerDesc::default()).unwrap();
    let output = gpu
        .create_buffer(20, vk::BufferUsageFlags::STORAGE_BUFFER, true)
        .unwrap();
    let out = gpu.storage_buffer(output).unwrap();
    let shader = r#"
struct DepthEntry {ulong unused; depth2d<float> image; ulong metadata;};
kernel void compareDepth(constant TopLevel& t [[buffer(2)]]){
    device DepthEntry* images=(device DepthEntry*)t.textures;
    float value=images[t.root[0]].image.sample_compare(t.comparison_samplers[t.root[1]].state,float2(0.5),0.5);
    float regular=t.textures[t.root[4]].texture.sample(t.samplers[t.root[5]].state,float2(0.5)).x;
    t.buffers[t.root[2]].data[t.root[3]]=as_type<uint>(value+regular);
}
"#;
    let p = gpu.msl_pipeline_from(
        shader,
        &[(cooked::Stage::Compute, "compareDepth")],
        &cooked::RenderState::default(),
        Some([1, 1, 1]),
    );
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::UploadImage {
        source: input,
        destination: depth,
    });
    rec.push(Command::UploadImage {
        source: input,
        destination: color,
    });
    rec.barriers([Barrier {
        resource: Resource::Image(color),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    rec.barriers([Barrier {
        resource: Resource::Image(depth),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    rec.push(Command::Pipeline(p));
    for (i, compare) in [
        CompareOp::Less,
        CompareOp::LessEqual,
        CompareOp::Greater,
        CompareOp::GreaterEqual,
        CompareOp::Always,
    ]
    .into_iter()
    .enumerate()
    {
        let sampler = gpu
            .sampler(&SamplerDesc {
                compare: Some(compare),
                min: Filter::Nearest,
                mag: Filter::Nearest,
                ..Default::default()
            })
            .unwrap();
        rec.push(Command::Constants(RootConstants {
            words: [
                image.index,
                sampler.index,
                out.index,
                i as u32,
                regular_image.index,
                regular.index,
                0,
                0,
            ],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
    }
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    let result: Vec<_> = gpu
        .read(output)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    assert_eq!(result, [0.25, 0.25, 1.25, 1.25, 1.25]);
}
