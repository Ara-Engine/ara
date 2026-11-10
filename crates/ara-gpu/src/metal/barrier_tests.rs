//! A dependency discovered after an unrelated copy can name an earlier encoder.
use super::*;
use crate::{
    commands::Command,
    cooked::{RenderState, Stage},
};

#[test]
fn late_copy_dependency_waits_for_a_prior_timed_compute_encoder() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    gpu.set_pass_timing(true);
    let target = gpu
        .create_buffer(
            4,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            true,
        )
        .unwrap();
    gpu.upload(target, &0u32.to_le_bytes()).unwrap();
    let slot = gpu.storage_buffer(target).unwrap();
    let unrelated = gpu
        .create_buffer(4, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .unwrap();
    gpu.upload(unrelated, &17u32.to_le_bytes()).unwrap();
    let shader = r#"
kernel void produce(constant TopLevel& t [[buffer(2)]]) {
    uint x=t.root[2];
    for(uint i=0;i<t.root[1];++i) {x=(x^(x>>13u))*1664525u+1013904223u;}
    t.buffers[t.root[0]].data[0]=x;
}
"#;
    let pipeline = gpu.msl_pipeline_from(
        shader,
        &[(Stage::Compute, "produce")],
        &RenderState::default(),
        Some([1, 1, 1]),
    );
    let mut rec = gpu.begin(Queue::Graphics);
    let mut snapshots = Vec::new();
    let mut before = Access::HostWrite;
    for seed in 1..=8u32 {
        rec.barriers([Barrier {
            resource: Resource::Buffer(target),
            before,
            after: Access::Storage,
            handoff: None,
        }]);
        rec.push(Command::BeginPass("timed producer".into()));
        rec.push(Command::Pipeline(pipeline));
        rec.push(Command::Constants(RootConstants {
            words: [slot.index, 8192, seed, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        rec.push(Command::EndPass);
        let unrelated_copy = gpu.create_readback_buffer(4).unwrap();
        // The timestamp ended the producer's encoder. This unrelated copy opens
        // another encoder without owing any dependency on that producer.
        rec.barriers([
            Barrier {
                resource: Resource::Buffer(unrelated),
                before: Access::HostWrite,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(unrelated_copy),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        rec.push(Command::CopyBufferRange {
            source: unrelated,
            source_offset: 0,
            destination: unrelated_copy,
            destination_offset: 0,
            size: 4,
        });
        let read = gpu.create_readback_buffer(4).unwrap();
        // This dependency needs both the current encoder's barrier and a queue
        // barrier: the shader producer is in the previously ended encoder.
        rec.barriers([
            Barrier {
                resource: Resource::Buffer(target),
                before: Access::Storage,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(read),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        rec.push(Command::CopyBufferRange {
            source: target,
            source_offset: 0,
            destination: read,
            destination_offset: 0,
            size: 4,
        });
        rec.barriers([Barrier {
            resource: Resource::Buffer(read),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        let mut expected = seed;
        for _ in 0..8192 {
            expected = (expected ^ (expected >> 13))
                .wrapping_mul(1664525)
                .wrapping_add(1013904223);
        }
        snapshots.push((read, expected, unrelated_copy));
        before = Access::CopySource;
    }
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    for (read, expected, unrelated_copy) in snapshots {
        assert_eq!(gpu.read(read).unwrap(), expected.to_le_bytes());
        gpu.retire(Resource::Buffer(read)).unwrap();
        gpu.retire(Resource::Buffer(unrelated_copy)).unwrap();
    }
    for buffer in [target, unrelated] {
        gpu.retire(Resource::Buffer(buffer)).unwrap();
    }
    gpu.retire_pipeline(pipeline).unwrap();
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}

#[test]
fn copy_dependency_survives_the_timed_consumer_opening_a_new_encoder() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    gpu.set_pass_timing(true);
    let words = 4 * 1024 * 1024;
    let pattern = 0x1234_5678u32;
    let data = vec![pattern; words];
    let bytes = bytemuck::cast_slice(&data);
    let source = gpu
        .create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .unwrap();
    gpu.upload(source, bytes).unwrap();
    let zeros = vec![0; bytes.len()];
    let shader = r#"
kernel void consume(constant TopLevel& t [[buffer(2)]]) {
    device uint* p=t.buffers[t.root[0]].data;
    t.buffers[t.root[1]].data[0]=p[0]+p[t.root[2]/2]+p[t.root[2]-1];
}
"#;
    let pipeline = gpu.msl_pipeline_from(
        shader,
        &[(Stage::Compute, "consume")],
        &RenderState::default(),
        Some([1, 1, 1]),
    );
    let mut rec = gpu.begin(Queue::Graphics);
    let mut snapshots = Vec::new();
    rec.barriers([Barrier {
        resource: Resource::Buffer(source),
        before: Access::HostWrite,
        after: Access::CopySource,
        handoff: None,
    }]);
    for _ in 0..8 {
        let copied = gpu
            .create_buffer(
                bytes.len() as u64,
                vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )
            .unwrap();
        gpu.upload(copied, &zeros).unwrap();
        let output = gpu
            .create_buffer(4, vk::BufferUsageFlags::STORAGE_BUFFER, true)
            .unwrap();
        let input_slot = gpu.storage_buffer(copied).unwrap().index;
        let output_slot = gpu.storage_buffer(output).unwrap().index;
        rec.barriers([Barrier {
            resource: Resource::Buffer(copied),
            before: Access::HostWrite,
            after: Access::CopyDestination,
            handoff: None,
        }]);
        rec.push(Command::CopyBufferRange {
            source,
            source_offset: 0,
            destination: copied,
            destination_offset: 0,
            size: bytes.len() as u64,
        });
        rec.barriers([Barrier {
            resource: Resource::Buffer(copied),
            before: Access::CopyDestination,
            after: Access::ShaderRead,
            handoff: None,
        }]);
        // The barrier was encountered in the copy's encoder. Timestamping opens
        // another encoder for the consumer, which still owes the queue dependency.
        rec.push(Command::BeginPass("timed transfer consumer".into()));
        rec.push(Command::Pipeline(pipeline));
        rec.push(Command::Constants(RootConstants {
            words: [input_slot, output_slot, words as u32, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        rec.push(Command::EndPass);
        rec.barriers([Barrier {
            resource: Resource::Buffer(output),
            before: Access::Storage,
            after: Access::HostRead,
            handoff: None,
        }]);
        snapshots.push((copied, output));
    }
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    for (copied, output) in snapshots {
        assert_eq!(
            gpu.read(output).unwrap(),
            pattern.wrapping_mul(3).to_le_bytes()
        );
        gpu.retire(Resource::Buffer(copied)).unwrap();
        gpu.retire(Resource::Buffer(output)).unwrap();
    }
    gpu.retire(Resource::Buffer(source)).unwrap();
    gpu.retire_pipeline(pipeline).unwrap();
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}
