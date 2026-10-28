use super::*;
use ara_gpu::{Barrier, RootConstants, vulkan::Attachment};
use std::time::Instant;

pub(super) fn scene() -> NativeScene {
    let mut materials = vec![instanced::Material::default(); 3];
    for material in &mut materials {
        material.pbr.textures = [u32::MAX; 4];
    }
    materials[1].pbr.flags[0] = 1;
    materials[2].pbr.flags[0] = 2;
    materials[2].pbr.base_color[3] = 0.3;
    let mut instances: Vec<_> = [-0.5, 0.5, 0.]
        .into_iter()
        .enumerate()
        .map(|(i, x)| instanced::Instance {
            entity: i as u64,
            name: format!("caster-{i}"),
            geometry: 0,
            material: i,
            world: Mat4::from_translation(Vec3::new(x, 0., 0.)),
            previous_world: Mat4::IDENTITY,
        })
        .collect();
    instances.extend((0..512).map(|i| instanced::Instance {
        entity: (i + 100) as u64,
        name: format!("distant-{i}"),
        geometry: 0,
        material: 0,
        world: Mat4::from_translation(Vec3::new(1000. + i as f32 * 2., 0., 0.)),
        previous_world: Mat4::IDENTITY,
    }));
    NativeScene {
        geometry: vec![instanced::Geometry {
            key: [71; 32],
            revision: 0,
            deforming: false,
            vertices: [(-0.2, -0.2), (0.2, -0.2), (0., 0.2)]
                .into_iter()
                .map(|(x, y)| instanced::Vertex {
                    position: [x, y, 0.5],
                    normal: [0., 0., 1.],
                    tangent: [1., 0., 0., 1.],
                    uv: [0.; 2],
                    uv1: [0.; 2],
                    terrain_weights: 0,
                    reserved: 0,
                })
                .collect(),
            lods: vec![instanced::Lod {
                indices: vec![0, 1, 2],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.2, -0.2, 0.5), Vec3::new(0.2, 0.2, 0.5)],
        }],
        materials,
        instances,
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ONE,
        environment: vec![[0.02, 0.02, 0.02, 1.]],
        environment_size: [1, 1],
    }
}
fn selected(renderer: &Renderer, pass: usize) -> Vec<u32> {
    let native = renderer.native.as_ref().unwrap();
    native.culling.draws[pass]
        .iter()
        .flat_map(|batch| {
            native.culling.visible[batch.first as usize..(batch.first + batch.count) as usize]
                .iter()
                .map(|row| row[0])
        })
        .collect()
}

#[test]
fn local_shadow_slots_keep_faces_independent_cache_casters_and_record_depth() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/local-shadow-slots")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let mut scene = scene();
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [128, 96],
        &[],
        root.join("gpu"),
    )?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 1.,
    };
    renderer.render_view(&camera, 1., ViewMode::Albedo)?;
    assert_eq!(
        renderer.native.as_ref().unwrap().culling.draws.len(),
        culling::BASE_PASSES
    );
    let matrices = [
        Mat4::IDENTITY,
        Mat4::from_translation(Vec3::new(-1000., 0., 0.)),
    ];
    let began = Instant::now();
    for _ in 0..100 {
        for matrix in matrices {
            renderer.cull_native_shadow(matrix)?;
        }
    }
    let far_us = began.elapsed().as_secs_f64() * 1e6;
    let baseline = renderer.native.as_ref().unwrap().culling.visible.clone();
    renderer.reserve_native_shadow_passes(2)?;
    assert_eq!(
        &renderer.native.as_ref().unwrap().culling.visible[..baseline.len()],
        &baseline
    );
    let passes = [
        renderer.cull_native_shadow_pass(0, matrices[0])?,
        renderer.cull_native_shadow_pass(1, matrices[1])?,
    ];
    // Local shadow passes follow the base passes, in slot order.
    assert_eq!(passes, [culling::BASE_PASSES, culling::BASE_PASSES + 1]);
    assert_eq!(selected(&renderer, passes[0]), [0, 1]);
    assert_eq!(selected(&renderer, passes[1]), [3]);
    let cache_tests = renderer.native_frame_work().unwrap().cull_tests;
    let usage = renderer.gpu.descriptor_usage();
    let began = Instant::now();
    for _ in 0..100 {
        renderer.reserve_native_shadow_passes(2)?;
        for (slot, matrix) in matrices.into_iter().enumerate() {
            renderer.cull_native_shadow_pass(slot, matrix)?;
        }
    }
    let cached_us = began.elapsed().as_secs_f64() * 1e6;
    assert_eq!(
        renderer.native_frame_work().unwrap().cull_tests,
        cache_tests
    );
    assert_eq!(renderer.gpu.descriptor_usage(), usage);
    assert!(renderer.cull_native_shadow_pass(2, matrices[0]).is_err());
    assert!(renderer.reserve_native_shadow_passes(usize::MAX).is_err());
    assert_eq!(renderer.gpu.descriptor_usage(), usage);

    let base = renderer.frame_data(
        &camera.resolve([128, 96])?,
        1.,
        true,
        ViewMode::Albedo,
        HISTORY0,
    );
    let mut frames = Vec::new();
    let mut depths = Vec::new();
    let mut readbacks = Vec::new();
    for matrix in matrices {
        let buffer = renderer.gpu.create_buffer(
            std::mem::size_of::<frame::Frame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let mut frame = base;
        frame.shadows[0] = matrix;
        renderer.gpu.upload(buffer, bytemuck::bytes_of(&frame))?;
        frames.push((buffer, renderer.gpu.storage_buffer(buffer)?.index));
        depths.push(renderer.gpu.create_image(
            64,
            64,
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )?);
        readbacks.push(renderer.gpu.create_buffer(
            64 * 64 * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?);
    }
    let mut recorder = renderer.gpu.begin(Queue::Graphics);
    renderer
        .native
        .as_ref()
        .unwrap()
        .resources(&mut |resource, before| {
            // The geometry pool stays in Geometry (index and shader reads).
            if before != Access::ShaderRead && before != Access::Geometry {
                recorder.barriers([Barrier {
                    resource,
                    before,
                    after: Access::ShaderRead,
                    handoff: None,
                }]);
            }
        });
    for &(buffer, _) in &frames {
        recorder.barriers([Barrier {
            resource: Resource::Buffer(buffer),
            before: Access::HostWrite,
            after: Access::ShaderRead,
            handoff: None,
        }]);
    }
    for slot in 0..2 {
        recorder.push(Command::BeginPass(format!("NativeShadow.Face{slot}")));
        recorder.barriers([Barrier {
            resource: Resource::Image(depths[slot]),
            before: Access::Undefined,
            after: Access::Depth,
            handoff: None,
        }]);
        recorder.push(Command::BeginRendering {
            colors: Vec::new(),
            depth: Some(Attachment {
                image: depths[slot],
                clear: Some([0.; 4]),
            }),
            size: [64, 64],
        });
        recorder.push(Command::Viewport {
            origin: [0, 0],
            size: [64, 64],
        });
        let mut root = RootConstants::default();
        root.words[0] = frames[slot].1;
        renderer
            .native
            .as_ref()
            .unwrap()
            .record(&mut recorder, root, passes[slot]);
        recorder.push(Command::EndRendering);
        recorder.barriers([
            Barrier {
                resource: Resource::Image(depths[slot]),
                before: Access::Depth,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(readbacks[slot]),
                before: Access::HostRead,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        recorder.push(Command::ReadImage {
            source: depths[slot],
            destination: readbacks[slot],
        });
        recorder.barriers([Barrier {
            resource: Resource::Buffer(readbacks[slot]),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        recorder.push(Command::EndPass);
    }
    let point = renderer.gpu.submit(recorder.finish(), &[])?;
    renderer.gpu.resolve(point)?;
    let mut counts = Vec::new();
    for (slot, &buffer) in readbacks.iter().enumerate() {
        let bytes = renderer.gpu.read(buffer)?;
        let values: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        counts.push(values.iter().filter(|&&depth| depth > 0.).count());
        assert_eq!(
            values[32 * 64 + 32] > 0.,
            slot == 1,
            "blend-only centre must not cast a shadow"
        );
        image::GrayImage::from_raw(64, 64, values.iter().map(|&v| (v * 255.) as u8).collect())
            .unwrap()
            .save(root.join(format!("face-{slot}.png")))?;
    }
    assert!(
        counts[1] > 0 && counts[0] > counts[1] * 3 / 2,
        "both opaque and masked casters must reach face 0: {counts:?}"
    );
    for image in depths {
        renderer.gpu.retire(Resource::Image(image))?;
    }
    for buffer in readbacks
        .into_iter()
        .chain(frames.into_iter().map(|(b, _)| b))
    {
        renderer.gpu.retire(Resource::Buffer(buffer))?;
    }
    renderer.gpu.collect()?;
    scene.instances[0].world = Mat4::from_translation(Vec3::new(100., 0., 0.));
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            instances: vec![0],
            ..Default::default()
        },
    )?;
    renderer.render_view(&camera, 1., ViewMode::Albedo)?;
    renderer.cull_native_shadow_pass(0, matrices[0])?;
    assert_eq!(selected(&renderer, passes[0]), [1]);
    scene.materials[1].pbr.flags[0] = 2;
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            materials: vec![1],
            ..Default::default()
        },
    )?;
    renderer.render_view(&camera, 1., ViewMode::Albedo)?;
    renderer.cull_native_shadow_pass(0, matrices[0])?;
    assert!(
        selected(&renderer, passes[0]).is_empty(),
        "masked-to-blended edits invalidate the cached caster list"
    );
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
    // The moved caster's cascade patches reserved one slot per cascade and the
    // local-face patch slots (look::local::PATCHES) past the two local faces.
    assert_eq!(
        renderer.native.as_ref().unwrap().culling.draws.len(),
        culling::BASE_PASSES + 4 + crate::look::local::PATCHES
    );
    assert_eq!(renderer.cull_native_shadow_pass(1, matrices[1])?, passes[1]);
    assert_eq!(selected(&renderer, passes[1]), [3]);
    assert_eq!(renderer.cull_native_shadow_pass(0, matrices[0])?, passes[0]);
    assert!(selected(&renderer, passes[0]).is_empty());
    renderer.gpu.assert_clean()?;
    println!(
        "100 two-face culls: shared far list {far_us:.2} us, independent cached slots {cached_us:.2} us; depth counts {counts:?}; {}",
        root.display()
    );
    Ok(())
}
