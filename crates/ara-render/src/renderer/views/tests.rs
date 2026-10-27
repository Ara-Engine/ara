use super::*;
use crate::{
    instanced,
    look::gi::{ProbeSurfaces, STATIC_VECTORS, SURFACE_TEXELS},
};
use ara_gpu::{Barrier, commands::Command};
use std::sync::atomic::{AtomicUsize, Ordering};
pub(in crate::renderer) mod fixture;
mod preparation;

struct Textures(Arc<AtomicUsize>);
impl instanced::Textures for Textures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut GpuDevice) -> Result<instanced::TextureStats> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(Default::default())
    }
    fn refine(&mut self, _: &mut GpuDevice) -> Result<instanced::TextureStats> {
        Ok(Default::default())
    }
    fn binding(&self) -> Option<instanced::TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

#[test]
fn secondary_view_preserves_primary_pixels_history_motion_and_shared_gi() -> Result<()> {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/secondary-view");
    std::fs::create_dir_all(&root)?;
    let initializes = Arc::new(AtomicUsize::new(0));
    let mut renderer = Renderer::new_native(
        &fixture::scene(),
        Box::new(Textures(Arc::clone(&initializes))),
        [128, 80],
        &[],
        root.join("cache"),
    )?;
    renderer.set_probe_surfaces(Some(&ProbeSurfaces {
        layout: crate::look::gi::ProbeLayout::single(crate::look::gi::ProbeLattice {
            origin: [-1.; 3],
            spacing: [2.; 3],
            counts: [2; 3],
        }),
        texels: vec![[0; 4]; 8 * SURFACE_TEXELS],
        statics: vec![[0.; 4]; 8 * STATIC_VECTORS],
    }))?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 55_f32.to_radians(),
    };
    renderer.set_look(crate::look::Look {
        sky: Some(crate::look::Sky::default()),
        fog: Some(crate::look::Fog::default()),
        auto_exposure: Some(crate::look::AutoExposure::default()),
        bloom: 0.05,
        ..Default::default()
    })?;
    for _ in 0..4 {
        renderer.render(&camera, 1.)?;
    }
    let indices = [OUTPUT, HISTORY0, HISTORY1, ID];
    let mut before = Vec::new();
    for index in indices {
        before.push(pixels(
            &mut renderer.gpu,
            renderer.targets.images[index],
            renderer.targets.states[index],
            [128, 80],
            if (HISTORY0..=HISTORY1).contains(&index) {
                8
            } else {
                4
            },
        )?);
    }
    let handles = renderer.targets.images.to_vec();
    let history = renderer.targets.history;
    let previous = renderer.previous_vp;
    let frame_number = renderer.frame_number;
    let frame_bytes = renderer.gpu.read(renderer.frame)?;
    let scene_hash = renderer.scene_hash();
    let gi_before = gi_resources(&renderer);
    let descriptors = renderer.gpu.descriptor_usage();
    let initializes_before = initializes.load(Ordering::Relaxed);
    let secondary_camera = CameraSpec::LookAt {
        position: Vec3::new(0.5, 0.15, 2.2),
        target: Vec3::ZERO,
        vertical_fov: 45_f32.to_radians(),
    };
    let quality = crate::look::Quality {
        ambient_occlusion: Some(crate::look::AmbientOcclusion::default()),
        reflections: Some(crate::look::Reflections::default()),
        shadows: crate::look::Shadows {
            cascades: 1,
            resolution: 256,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut view = renderer.create_view()?;
    assert_eq!(
        renderer.gpu.descriptor_usage(),
        descriptors,
        "hidden views allocate nothing"
    );
    let output = renderer.render_secondary(
        &mut view,
        &ViewRequest {
            camera: &secondary_camera,
            size: [80, 56],
            render_size: [80, 56],
            quality,
            exposure: 1.,
        },
    )?;
    assert_ne!(output.image, renderer.output());
    assert_eq!(output.size, [80, 56]);
    let secondary = pixels(
        &mut renderer.gpu,
        output.image,
        State {
            access: Access::CopySource,
            queue: Queue::Graphics,
        },
        output.size,
        4,
    )?;
    image::save_buffer(
        root.join("secondary.png"),
        &secondary,
        80,
        56,
        image::ColorType::Rgba8,
    )?;
    image::save_buffer(
        root.join("primary.png"),
        &before[0],
        128,
        80,
        image::ColorType::Rgba8,
    )?;
    assert!(
        secondary
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| u16::from(pixel[0]) > u16::from(pixel[2]) + 10),
        "secondary draws native material"
    );
    for _ in 0..3 {
        renderer.submit_secondary(
            &mut view,
            &ViewRequest {
                camera: &secondary_camera,
                size: [80, 56],
                render_size: [80, 56],
                quality,
                exposure: 1.,
            },
            |_, _| Ok(true),
        )?;
        renderer.wait_scene_frame()?;
        assert!(renderer.view_output(&view)?.is_some());
    }
    let completed = renderer
        .view_output(&view)?
        .context("completed async camera")?;
    let async_pixels = pixels(
        &mut renderer.gpu,
        completed.image,
        State {
            access: Access::CopySource,
            queue: Queue::Graphics,
        },
        completed.size,
        4,
    )?;
    image::save_buffer(
        root.join("secondary-async.png"),
        &async_pixels,
        80,
        56,
        image::ColorType::Rgba8,
    )?;
    assert!(
        async_pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| u16::from(p[0]) > u16::from(p[2]) + 10),
        "retained async output must contain the native material"
    );
    let resized = renderer.render_secondary(
        &mut view,
        &ViewRequest {
            camera: &secondary_camera,
            size: [64, 48],
            render_size: [48, 36],
            quality: crate::look::Quality {
                shadows: crate::look::Shadows {
                    resolution: 512,
                    ..quality.shadows
                },
                ..quality
            },
            exposure: 0.8,
        },
    )?;
    assert_eq!(resized.size, [64, 48]);
    assert_ne!(resized.image, output.image);
    assert!(renderer.gpu.image_view(output.image).is_err());
    assert_eq!(renderer.targets.images, handles);
    assert_eq!(renderer.targets.history, history);
    assert_eq!(renderer.previous_vp, previous);
    assert_eq!(renderer.frame_number, frame_number);
    assert_eq!(renderer.gpu.read(renderer.frame)?, frame_bytes);
    assert_eq!(renderer.scene_hash(), scene_hash);
    assert_eq!(
        gi_resources(&renderer),
        gi_before,
        "both cameras share the same probe volume"
    );
    assert_eq!(
        initializes.load(Ordering::Relaxed),
        initializes_before,
        "no second texture residency initialization"
    );
    for (index, before) in indices.into_iter().zip(before) {
        assert_eq!(
            pixels(
                &mut renderer.gpu,
                renderer.targets.images[index],
                renderer.targets.states[index],
                [128, 80],
                if (HISTORY0..=HISTORY1).contains(&index) {
                    8
                } else {
                    4
                }
            )?,
            before,
            "primary channel {index} was modified"
        );
    }
    assert!(
        renderer
            .render_secondary(
                &mut view,
                &ViewRequest {
                    camera: &secondary_camera,
                    size: [0, 56],
                    render_size: [0, 56],
                    quality,
                    exposure: 1.
                }
            )
            .is_err()
    );
    drop(view);
    renderer.collect_view_resources()?;
    assert!(renderer.gpu.image_view(resized.image).is_err());
    assert_eq!(
        renderer.gpu.descriptor_usage(),
        descriptors,
        "all secondary targets/rows/look resources retire"
    );
    renderer.render(&camera, 1.)?;
    assert_eq!(
        renderer.frame_number,
        frame_number + 1,
        "primary AA continues without reset"
    );
    let mut changed = fixture::scene();
    changed.geometry[0].vertices[0].position[1] += 0.2;
    changed.geometry[0].revision += 1;
    renderer.update_native(&changed)?;
    let mut view = renderer.create_view()?;
    renderer.render_secondary(
        &mut view,
        &ViewRequest {
            camera: &secondary_camera,
            size: [80, 56],
            render_size: [80, 56],
            quality,
            exposure: 1.,
        },
    )?;
    renderer.render(&camera, 1.)?;
    let motion = pixels(
        &mut renderer.gpu,
        renderer.targets.images[MOTION],
        renderer.targets.states[MOTION],
        [128, 80],
        4,
    )?;
    assert!(
        motion.as_chunks::<4>().0.iter().any(|pixel| {
            [
                u16::from_le_bytes([pixel[0], pixel[1]]),
                u16::from_le_bytes([pixel[2], pixel[3]]),
            ]
            .iter()
            .any(|bits| bits & 0x7fff >= 0x1400)
        }),
        "secondary rendering must not consume the primary skin's previous slot"
    );
    renderer.release_view(view)?;
    renderer.gpu.assert_clean()?;
    Ok(())
}

#[test]
fn secondary_view_alone_drives_shared_probe_delivery_without_drawing_the_primary() -> Result<()> {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/secondary-only");
    let initializes = Arc::new(AtomicUsize::new(0));
    let mut renderer = Renderer::new_native(
        &fixture::scene(),
        Box::new(Textures(Arc::clone(&initializes))),
        [128, 80],
        &[],
        root.join("cache"),
    )?;
    renderer.set_probe_surfaces(Some(&ProbeSurfaces {
        layout: crate::look::gi::ProbeLayout::single(crate::look::gi::ProbeLattice {
            origin: [-1.; 3],
            spacing: [2.; 3],
            counts: [2; 3],
        }),
        texels: vec![[0; 4]; 8 * SURFACE_TEXELS],
        statics: vec![[0.; 4]; 8 * STATIC_VECTORS],
    }))?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 50_f32.to_radians(),
    };
    let quality = crate::look::Quality {
        shadows: crate::look::Shadows {
            cascades: 1,
            resolution: 256,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut view = renderer.create_view()?;
    for _ in 0..16 {
        renderer.render_secondary(
            &mut view,
            &ViewRequest {
                camera: &camera,
                size: [80, 56],
                render_size: [80, 56],
                quality,
                exposure: 1.,
            },
        )?;
    }
    assert!(!renderer.presentation_ready());
    assert_eq!(renderer.targets.history, None);
    assert_eq!(renderer.frame_number, 0);
    let gi = renderer.look.gi.as_ref().context("shared probe volume")?;
    assert!(!gi.fresh && gi.received == 8 && gi.converge == 0 && gi.weight > 0.);
    assert!(renderer.look.probe_writes.is_empty());
    assert_eq!(initializes.load(Ordering::Relaxed), 1);
    renderer.release_view(view)?;
    renderer.gpu.assert_clean()?;
    Ok(())
}

fn gi_resources(renderer: &Renderer) -> Option<(Buffer, Buffer, Image, [Image; 2])> {
    renderer
        .look
        .gi
        .as_ref()
        .map(|gi| (gi.cache, gi.meta, gi.moments, gi.irradiance))
}

fn pixels(
    gpu: &mut GpuDevice,
    image: Image,
    state: State,
    size: [u32; 2],
    stride: u64,
) -> Result<Vec<u8>> {
    let buffer = gpu.create_buffer(
        u64::from(size[0]) * u64::from(size[1]) * stride,
        vk::BufferUsageFlags::TRANSFER_DST,
        true,
    )?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([
        Barrier {
            resource: Resource::Image(image),
            before: state.access,
            after: Access::CopySource,
            handoff: None,
        },
        Barrier {
            resource: Resource::Buffer(buffer),
            before: Access::Undefined,
            after: Access::CopyDestination,
            handoff: None,
        },
    ]);
    commands.push(Command::ReadImage {
        source: image,
        destination: buffer,
    });
    commands.barriers([
        Barrier {
            resource: Resource::Image(image),
            before: Access::CopySource,
            after: state.access,
            handoff: None,
        },
        Barrier {
            resource: Resource::Buffer(buffer),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        },
    ]);
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    let bytes = gpu.read(buffer)?;
    gpu.retire(Resource::Buffer(buffer))?;
    gpu.collect()?;
    Ok(bytes)
}
