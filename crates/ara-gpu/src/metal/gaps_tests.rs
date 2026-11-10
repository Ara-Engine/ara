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
