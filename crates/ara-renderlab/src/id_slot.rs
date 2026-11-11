//! The id image crosses on its own ring beside the colour frame. The bytes below
//! are what both sides must hold: little-endian RGBA8, `0` and `0xFFFF_FFFF` in the
//! first two texels, and the frame number in the alpha of every other texel so a
//! stale frame cannot match.
//!
//! The GPU test (`id_slot_matches_across_resize`) is one command and uses the
//! shared GPU. Do not run it outside the lease.

/// Packed id at one texel. `R` is bits 0–7, `G` 8–15, `B` 16–23, `A` 24–31.
fn id_texel(x: u32, y: u32, width: u32, frame: u32) -> [u8; 4] {
    if x == 0 && y == 0 {
        return [0; 4];
    }
    if x == 1 && y == 0 {
        return [0xff; 4];
    }
    let n = x + y * width;
    [
        n.wrapping_mul(17).wrapping_add(3) as u8,
        n.wrapping_mul(29).wrapping_add(5) as u8,
        n.wrapping_mul(41).wrapping_add(7) as u8,
        frame as u8,
    ]
}

/// Colour bytes for the same frame: red carries the frame number.
fn colour_texel(x: u32, y: u32, frame: u32) -> [u8; 4] {
    [frame as u8, (x & 255) as u8, (y & 255) as u8, 0x5a]
}

fn pattern(size: [u32; 2], frame: u32) -> (Vec<u8>, Vec<u8>) {
    let (width, height) = (size[0], size[1]);
    let mut colour = Vec::with_capacity((width * height * 4) as usize);
    let mut id = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            colour.extend(colour_texel(x, y, frame));
            id.extend(id_texel(x, y, width, frame));
        }
    }
    (colour, id)
}

#[test]
fn id_pattern_uses_all_channels_and_the_reserved_values() {
    let size = [64, 40];
    let (colour, id) = pattern(size, 7);
    assert_eq!(&id[0..4], &[0, 0, 0, 0]);
    assert_eq!(&id[4..8], &[0xff, 0xff, 0xff, 0xff]);
    assert_eq!(colour[0], 7, "colour red is the frame");
    assert_eq!(id[11], 7, "alpha of texel (2, 0) is the frame");
    let mut seen = [false; 4];
    for texel in id.as_chunks::<4>().0.iter().skip(2) {
        for (channel, byte) in texel.iter().enumerate() {
            if *byte != 0 {
                seen[channel] = true;
            }
        }
    }
    assert_eq!(seen, [true, true, true, true]);
    let (_, other) = pattern(size, 8);
    assert_ne!(id, other);
    assert_ne!(pattern([17, 13], 7).1.len(), id.len());
}

#[cfg(windows)]
mod gpu {
    use super::pattern;
    use crate::shared::SharedHandoff;
    use ara_gpu::{
        Access, Barrier, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk,
    };
    use std::path::PathBuf;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::sync::Arc::new(Unpark(std::thread::current())).into();
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::thread::park(),
            }
        }
    }

    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }

    fn scratch() -> PathBuf {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
        root.join("id-slot")
    }

    fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, tight: &[u8], size: [u32; 2]) {
        let row = size[0] * 4;
        let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let mut padded = vec![0u8; (stride * size[1]) as usize];
        for y in 0..size[1] as usize {
            let from = y * row as usize;
            let to = y * stride as usize;
            padded[to..to + row as usize].copy_from_slice(&tight[from..from + row as usize]);
        }
        queue.write_texture(
            texture.as_image_copy(),
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size[1]),
            },
            texture.size(),
        );
    }

    fn source(device: &wgpu::Device, size: [u32; 2], label: &str) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn settle(gpu: &mut GpuDevice, image: Image) -> ara_gpu::Result<()> {
        let mut commands = gpu.begin(Queue::Graphics);
        for (before, after) in [
            (Access::Undefined, Access::CopyDestination),
            (Access::CopyDestination, Access::CopySource),
        ] {
            commands.barriers([Barrier {
                resource: Resource::Image(image),
                before,
                after,
                handoff: None,
            }]);
        }
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.wait(point)
    }

    fn destination(gpu: &mut GpuDevice, size: [u32; 2]) -> ara_gpu::Result<Image> {
        let image = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::SAMPLED,
        )?;
        settle(gpu, image)?;
        Ok(image)
    }

    fn read_image(gpu: &mut GpuDevice, image: Image, size: [u32; 2]) -> ara_gpu::Result<Vec<u8>> {
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
        let buffer = gpu.create_readback_buffer(bytes)?;
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::ReadImage {
            source: image,
            destination: buffer,
        });
        commands.barriers([Barrier {
            resource: Resource::Buffer(buffer),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.wait(point)?;
        let pixels = gpu.read(buffer)?;
        gpu.retire(Resource::Buffer(buffer))?;
        gpu.collect()?;
        Ok(pixels)
    }

    fn assert_exact(got: &[u8], want: &[u8], what: &str, size: [u32; 2]) {
        assert_eq!(got.len(), want.len(), "{what} byte count at {size:?}");
        if let Some(index) = got.iter().zip(want).position(|(got, want)| got != want) {
            let pixel = (index / 4) as u32;
            let x = pixel % size[0];
            let y = pixel / size[0];
            panic!(
                "{what} at ({x}, {y}) byte {}: got {:02x}, want {:02x}",
                index % 4,
                got[index],
                want[index]
            );
        }
    }

    fn frames(
        handoff: &mut SharedHandoff,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu: &mut GpuDevice,
        size: [u32; 2],
        colour_dst: Image,
        id_dst: Image,
    ) -> anyhow::Result<()> {
        let colour = source(device, size, "synthetic colour");
        let id = source(device, size, "synthetic id");
        // The real id texture replaces `id` here and nowhere else.
        handoff.set_id(id.clone());
        for frame in 1..=3 {
            let (colour_bytes, id_bytes) = pattern(size, frame);
            upload(queue, &colour, &colour_bytes, size);
            upload(queue, &id, &id_bytes, size);
            handoff.publish_textures(device, queue, &colour)?;
            handoff.deliver(gpu, colour_dst)?;
            handoff.deliver_id(gpu, id_dst)?;
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            assert_exact(
                &read_image(gpu, colour_dst, size)?,
                &colour_bytes,
                &format!("colour frame {frame}"),
                size,
            );
            assert_exact(
                &read_image(gpu, id_dst, size)?,
                &id_bytes,
                &format!("id frame {frame}"),
                size,
            );
        }
        Ok(())
    }

    /// Every texel read on the presenter equals the bytes written on the wgpu
    /// device, at two sizes and across one resize, for three frames each.
    #[test]
    fn id_slot_matches_across_resize() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = block_on(instance.request_adapter(&Default::default()))
            .expect("a Vulkan adapter for the id slot");
        let (device, queue) = block_on(adapter.request_device(&Default::default()))
            .expect("a wgpu device for the id slot");
        let dir = scratch();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let mut gpu = GpuDevice::new(&[], dir.join("presenter")).expect("presenter device");

        let small = [17, 13];
        let large = [64, 40];
        let mut handoff = SharedHandoff::on_device(&device, &gpu, small).expect("shared rings");
        let mut colour_dst = destination(&mut gpu, small).expect("colour target");
        let mut id_dst = destination(&mut gpu, small).expect("id target");
        frames(
            &mut handoff,
            &device,
            &queue,
            &mut gpu,
            small,
            colour_dst,
            id_dst,
        )
        .expect("small frames");

        gpu.retire(Resource::Image(colour_dst))
            .expect("retire colour");
        gpu.retire(Resource::Image(id_dst)).expect("retire id");
        gpu.collect().expect("collect");
        handoff = SharedHandoff::on_device(&device, &gpu, large).expect("resized rings");
        colour_dst = destination(&mut gpu, large).expect("resized colour target");
        id_dst = destination(&mut gpu, large).expect("resized id target");
        frames(
            &mut handoff,
            &device,
            &queue,
            &mut gpu,
            large,
            colour_dst,
            id_dst,
        )
        .expect("large frames");
        drop(handoff);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }
}
