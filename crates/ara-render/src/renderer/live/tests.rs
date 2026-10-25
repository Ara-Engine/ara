use super::*;
use crate::renderer::views::tests::fixture;
use crate::{capture::ViewMode, instanced};
use ara_gpu::vk::TaggedStructure;

struct Textures;
impl instanced::Textures for Textures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut VulkanDevice) -> Result<instanced::TextureStats> {
        Ok(Default::default())
    }
    fn refine(&mut self, _: &mut VulkanDevice) -> Result<instanced::TextureStats> {
        Ok(Default::default())
    }
    fn binding(&self) -> Option<instanced::TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut VulkanDevice) -> Result<()> {
        Ok(())
    }
}
struct Gate {
    device: ash::Device,
    semaphore: vk::Semaphore,
    opened: bool,
}
impl Gate {
    fn new(gpu: &VulkanDevice) -> Result<Self> {
        // SAFETY: New test-owned semaphore. Only this thread submits or signals it.
        unsafe {
            let device = gpu.context().device.clone();
            let mut kind =
                vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
            let semaphore = device
                .create_semaphore(&vk::SemaphoreCreateInfo::default().push(&mut kind), None)?;
            let gate = Self {
                device,
                semaphore,
                opened: false,
            };
            gate.device.queue_submit2(
                gpu.context().compute,
                &[vk::SubmitInfo2::default().wait_semaphore_infos(&[
                    vk::SemaphoreSubmitInfo::default()
                        .semaphore(semaphore)
                        .value(1)
                        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
                ])],
                vk::Fence::null(),
            )?;
            Ok(gate)
        }
    }
    fn open(&mut self) -> Result<()> {
        if !self.opened {
            // SAFETY: The only host signal for this semaphore; it releases compute.
            unsafe {
                self.device.signal_semaphore(
                    &vk::SemaphoreSignalInfo::default()
                        .semaphore(self.semaphore)
                        .value(1),
                )?;
            }
            self.opened = true;
        }
        Ok(())
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        let _ = self.open();
        // SAFETY: Also on assertion failure, signal precedes idle and destruction.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_semaphore(self.semaphore, None);
        }
    }
}

#[test]
fn scene_submission_retains_completed_color_ids_and_pick_while_gpu_is_blocked() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/async-scene");
    let mut scene = fixture::scene();
    let mut renderer = Renderer::new_native(&scene, Box::new(Textures), [128, 80], &[], root)?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 55f32.to_radians(),
    };
    renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?;
    renderer.wait_scene_frame()?;
    let previous = (renderer.output(), renderer.entity_channel());
    assert_eq!(renderer.entity_at([64, 40])?, Some(7));
    scene.instances[0].world = Mat4::from_translation(Vec3::new(1.8, 0., 0.));
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            instances: vec![0],
            ..Default::default()
        },
    )?;
    let mut gate = Gate::new(renderer.gpu())?;
    renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?;
    assert!(renderer.scene_frame_pending());
    assert!(
        !renderer.poll_scene_frame()?,
        "blocked GPU must return to its UI owner"
    );
    assert_eq!((renderer.output(), renderer.entity_channel()), previous);
    if renderer.gpu().context().graphics != renderer.gpu().context().scene_graphics {
        // The pick copies four bytes from the shown ID image on the UI queue,
        // rather than waiting for the new Scene or reading its unwritten IDs.
        assert_eq!(renderer.entity_at([64, 40])?, Some(7));
    }
    assert!(!renderer.submit_scene_frame(|_| panic!("a busy frame must coalesce"))?);
    renderer.resize([160, 100])?;
    assert_eq!(
        renderer.size(),
        [128, 80],
        "in-flight targets stay alive through resize"
    );
    gate.open()?;
    renderer.wait_scene_frame()?;
    assert_eq!(renderer.size(), [160, 100]);
    assert_eq!(renderer.entity_at([80, 50])?, None);
    assert_ne!((renderer.output(), renderer.entity_channel()), previous);
    renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?;
    renderer.wait_scene_frame()?;
    let last_good = (renderer.output(), renderer.entity_channel());
    let mut view = renderer.create_view()?;
    let game_camera = CameraSpec::LookAt {
        position: Vec3::new(1.8, 0., 3.),
        target: Vec3::new(1.8, 0., 0.),
        vertical_fov: 55f32.to_radians(),
    };
    let secondary = renderer.render_secondary(
        &mut view,
        &crate::ViewRequest {
            camera: &game_camera,
            size: [72, 44],
            render_size: [72, 44],
            quality: *renderer.quality(),
            exposure: 1.,
        },
    )?;
    assert_ne!(secondary.image, last_good.0);
    assert_eq!(renderer.gpu().image_extent(secondary.image)?, [72, 44]);
    assert_eq!((renderer.output(), renderer.entity_channel()), last_good);
    // SAFETY: render_secondary completed and published this immutable ID image.
    let id = unsafe {
        renderer
            .gpu_mut()
            .read_ui_pixel(secondary.entities, [36, 22])?
    };
    assert_eq!(
        f32::from_le_bytes(id),
        1.,
        "the secondary uses its own camera and IDs"
    );
    renderer.release_view(view)?;
    for _ in 0..8 {
        assert!(
            renderer
                .submit_scene_frame(|_| anyhow::bail!("rejected frame"))
                .is_err()
        );
        let discarded = renderer
            .live
            .as_ref()
            .expect("async owner")
            .pending
            .as_ref()
            .expect("failed frame")
            .output
            .color;
        renderer.wait_scene_frame()?;
        assert_eq!((renderer.output(), renderer.entity_channel()), last_good);
        assert!(
            renderer.gpu().native_image(discarded).is_err(),
            "failed frame resources retire"
        );
    }
    renderer.gpu().assert_clean()?;
    Ok(())
}
