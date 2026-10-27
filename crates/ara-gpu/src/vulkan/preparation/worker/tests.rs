use super::*;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

struct Release(Option<Sender<()>>);
impl Release {
    fn open(&mut self) {
        if let Some(send) = self.0.take() {
            let _ = send.send(());
        }
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}

fn request() -> ResourcePreparation {
    ResourcePreparation {
        buffers: vec![
            BufferPreparation {
                size: 4,
                usage: vk::BufferUsageFlags::STORAGE_BUFFER,
                host: true,
                data: vec![3, 4, 5, 6].into(),
            },
            BufferPreparation {
                size: 8,
                usage: vk::BufferUsageFlags::STORAGE_BUFFER,
                host: true,
                data: InitialData::deferred(|| {
                    assert_eq!(std::thread::current().name(), Some("ara-gpu-prepare"));
                    Ok(vec![6; 8])
                }),
            },
        ],
        images: vec![
            ImagePreparation {
                size: [4, 4],
                format: vk::Format::R8G8B8A8_UNORM,
                mip_levels: 2,
                usage: vk::ImageUsageFlags::SAMPLED,
            },
            ImagePreparation {
                size: [8, 4],
                format: vk::Format::D32_SFLOAT,
                mip_levels: 1,
                usage: vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            },
        ],
    }
}

fn wait(gpu: &mut VulkanDevice, id: PreparationId) -> Result<PreparedResources> {
    let started = Instant::now();
    loop {
        if let Some(resources) = gpu.poll_prepared_resources(id)? {
            return Ok(resources);
        }
        assert!(started.elapsed() < Duration::from_secs(5), "worker timeout");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn preparation_is_bounded_nonblocking_and_adopts_initialized_resources_once() -> anyhow::Result<()>
{
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/preparation");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let baseline = gpu.descriptor_usage();
    let (send, receive) = channel();
    let mut release = Release(Some(send));
    let (entered, started) = channel();
    let mut gate = Some(receive);
    gpu.preparation = Some(Worker::start(gpu.context.allocator(), move || {
        if let Some(gate) = gate.take() {
            let _ = entered.send(());
            let _ = gate.recv();
        }
    })?);
    let first = gpu.prepare_resources(request)?.expect("admitted");
    started.recv_timeout(Duration::from_secs(5))?;
    let cancelled = gpu.prepare_resources(request)?.expect("one queued");
    assert!(
        gpu.prepare_resources(|| panic!("full queue must not build input"))?
            .is_none()
    );
    let waits = gpu.context.host_wait_stats().calls;
    for _ in 0..8 {
        assert!(gpu.poll_prepared_resources(first)?.is_none());
    }
    assert_eq!(gpu.context.host_wait_stats().calls, waits);
    assert_eq!(gpu.descriptor_usage(), baseline);
    assert_eq!(gpu.buffers.values_mut().count(), 0);
    assert_eq!(gpu.images.values_mut().count(), 0);
    gpu.cancel_prepared_resources(cancelled)?;
    assert!(matches!(
        gpu.poll_prepared_resources(cancelled),
        Err(GpuError::StaleHandle)
    ));
    release.open();
    let resources = wait(&mut gpu, first)?;
    assert_eq!(gpu.read(resources.buffers[0])?, [3, 4, 5, 6]);
    assert_eq!(gpu.read(resources.buffers[1])?, [6; 8]);
    assert!(gpu.native_image(resources.images[0]).is_ok());
    assert_eq!(gpu.images.get(resources.images[0])?.raw.mip_levels, 2);
    assert_eq!(gpu.images.get(resources.images[1])?.raw.width, 8);
    assert_eq!(
        gpu.images.get(resources.images[1])?.raw.aspect,
        vk::ImageAspectFlags::DEPTH
    );
    assert!(matches!(
        gpu.poll_prepared_resources(first),
        Err(GpuError::StaleHandle)
    ));
    for buffer in resources.buffers {
        gpu.retire(Resource::Buffer(buffer))?;
    }
    for image in resources.images {
        gpu.retire(Resource::Image(image))?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        gpu.collect()?;
        if gpu.preparation.as_ref().expect("worker").count == 0 {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(gpu.buffers.values_mut().count(), 0);
    assert_eq!(gpu.images.values_mut().count(), 0);
    assert_eq!(gpu.descriptor_usage(), baseline);
    // Failure after a valid buffer allocation must publish nothing and release
    // that partial allocation on the worker, not during a later UI frame.
    let failed = gpu
        .prepare_resources(|| {
            let mut request = request();
            request.images[0].size = [0, 4];
            request
        })?
        .expect("admitted");
    assert!(matches!(wait(&mut gpu, failed), Err(GpuError::Contract(_))));
    assert_eq!(gpu.buffers.values_mut().count(), 0);
    assert_eq!(gpu.images.values_mut().count(), 0);
    drop(gpu.preparation.take());
    drop(gpu.disposal.take());
    gpu.assert_clean()?;
    Ok(())
}
