use super::*;
use ara_gpu::vulkan::{BufferPreparation, InitialData, ResourcePreparation};
use ara_render::instanced::Textures;
use std::{sync::mpsc, time::Duration};

fn wait(
    gpu: &mut VulkanDevice,
    next: &mut NativeResidency,
    old: Option<&dyn Textures>,
) -> Result<()> {
    let started = Instant::now();
    while !next.preallocate(gpu, old)? {
        ensure!(
            started.elapsed() < Duration::from_secs(10),
            "texture worker timeout"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
fn prepared_residency_preserves_mips_view_policy_and_rejected_generation_ownership() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/native-preparation");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let mut old = bank(1024, 1024);
    old.register([1; 32], source([8, 8], 4, 64))?;
    let color = Arc::new(NativeSource::from_cooked(
        write_ktx2(4, 4, TextureFormat::Bc7Srgb, &[vec![0; 16]])?.into(),
    )?);
    old.register([2; 32], Arc::clone(&color))?;
    wait(&mut gpu, &mut old, None)?;
    assert!(
        old.binding().is_none(),
        "allocation does not publish a table"
    );
    old.initialize(&mut gpu)?;
    let original = old.entries[0].resident.unwrap();
    let srgb_image = old.entries[1].resident.unwrap().image;
    let baseline = gpu.descriptor_usage();
    for _ in 0..8 {
        let mut rejected = bank(1024, 1024);
        rejected.register([1; 32], source([8, 8], 4, 64))?;
        rejected.register([3; 32], source([4, 4], 1, 192))?;
        wait(&mut gpu, &mut rejected, Some(&old))?;
        assert!(rejected.binding().is_none());
        // Abandoning an adopted allocation must not consume borrowed ownership.
        rejected.destroy(&mut gpu)?;
        assert_eq!(gpu.descriptor_usage(), baseline);
        assert_eq!(old.entries[0].resident.unwrap().image, original.image);
    }
    let mut next = bank(1024, 1024);
    next.register([3; 32], source([8, 8], 4, 192))?;
    next.request(NativeTextureId(0), 1, 1.)?;
    next.register([1; 32], source([8, 8], 4, 64))?;
    next.register_view([2; 32], color, NativeView::Raw)?;
    wait(&mut gpu, &mut next, Some(&old))?;
    assert!(next.register([4; 32], source([4, 4], 1, 128)).is_err());
    assert!(next.request(NativeTextureId(0), 0, 1.).is_err());
    assert_eq!(next.initialize_reusing(&mut gpu, &old)?.uploaded_bytes, 40);
    assert_eq!(next.entries[1].resident.unwrap().image, original.image);
    assert_ne!(next.entries[2].resident.unwrap().image, srgb_image);
    next.publish_reuse(&mut old)?;
    old.destroy(&mut gpu)?;
    for entry in &next.entries {
        let resident = entry.resident.unwrap();
        assert_eq!(
            blocks(&mut gpu, resident),
            entry.source.stage(resident.row.0[1])?
        );
    }
    next.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}

struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(send) = self.0.take() {
            let _ = send.send(());
        }
    }
}

#[test]
fn prepared_borrow_rebases_when_the_presented_generation_refines_its_images() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/native-preparation-rebase");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let mut old = bank(1024, 1024);
    let id = old.register([1; 32], source([8, 8], 4, 64))?;
    old.initialize(&mut gpu)?;
    let mut next = old.fork_sources(old.config)?;
    wait(&mut gpu, &mut next, Some(&old))?;
    let borrowed = next.entries[0].resident.unwrap().image;
    old.request(id, 1, 1.)?;
    old.refine(&mut gpu)?;
    assert_ne!(old.entries[0].resident.unwrap().image, borrowed);
    // A ready candidate must detect that refinement retired its borrowed image,
    // cancel its old table allocation and stage the newly required full tail.
    wait(&mut gpu, &mut next, Some(&old))?;
    assert_eq!(next.initialize_reusing(&mut gpu, &old)?.uploaded_bytes, 56);
    next.publish_reuse(&mut old)?;
    old.destroy(&mut gpu)?;
    assert_eq!(
        blocks(&mut gpu, next.entries[0].resident.unwrap()),
        next.entries[0].source.stage(0)?
    );
    next.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn pending_texture_allocations_poll_and_cancel_without_waiting_or_binding() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/native-preparation-cancel");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let (send, receive) = mpsc::channel();
    let release = Release(Some(send));
    let (entered, started) = mpsc::channel();
    let blocker = gpu
        .prepare_resources(|| ResourcePreparation {
            buffers: vec![BufferPreparation {
                size: 4,
                usage: vk::BufferUsageFlags::TRANSFER_SRC,
                host: true,
                data: InitialData::deferred(move || {
                    let _ = entered.send(());
                    let _ = receive.recv();
                    Ok(vec![0; 4])
                }),
            }],
            images: Vec::new(),
        })?
        .expect("first job admitted");
    started.recv_timeout(Duration::from_secs(5))?;
    let baseline = gpu.descriptor_usage();
    let waits = gpu.context().host_wait_stats().calls;
    let mut cancelled = bank(1024, 1024);
    cancelled.register([1; 32], source([8, 8], 4, 64))?;
    for _ in 0..8 {
        assert!(!cancelled.preallocate(&mut gpu, None)?);
    }
    assert!(cancelled.initialize(&mut gpu).is_err());
    cancelled.destroy(&mut gpu)?;
    let mut next = bank(1024, 1024);
    next.register([2; 32], source([4, 4], 1, 128))?;
    assert!(
        !next.preallocate(&mut gpu, None)?,
        "cancelled work still bounds admission"
    );
    assert_eq!(gpu.context().host_wait_stats().calls, waits);
    assert_eq!(gpu.descriptor_usage(), baseline);
    gpu.cancel_prepared_resources(blocker)?;
    drop(release);
    wait(&mut gpu, &mut next, None)?;
    next.initialize(&mut gpu)?;
    assert_eq!(
        blocks(&mut gpu, next.entries[0].resident.unwrap()),
        [128, 128, 0, 0, 0, 0, 0, 0]
    );
    next.destroy(&mut gpu)?;
    assert_eq!(gpu.descriptor_usage(), baseline);
    gpu.assert_clean()?;
    Ok(())
}
