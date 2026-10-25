use super::*;

fn scene() -> NativeScene {
    NativeScene {
        geometry: vec![instanced::Geometry {
            key: [29; 32],
            vertices: [(-0.5, -0.5), (0.5, -0.5), (0., 0.5), (0., -0.5)]
                .into_iter()
                .map(|(x, y)| instanced::Vertex {
                    position: [x, y, 0.],
                    normal: [0., 0., 1.],
                    uv: [0.; 2],
                    tangent: [1., 0., 0., 1.],
                    uv1: [0.; 2],
                    terrain_weights: 0,
                    reserved: 0,
                })
                .collect(),
            lods: vec![
                instanced::Lod {
                    indices: vec![0, 3, 2, 3, 1, 2],
                    error: 0.,
                },
                instanced::Lod {
                    indices: vec![0, 1, 2],
                    error: 0.12,
                },
            ],
            bounds: [Vec3::new(-0.5, -0.5, 0.), Vec3::new(0.5, 0.5, 0.)],
            deforming: false,
            revision: 0,
        }],
        instances: [1., 0.02]
            .into_iter()
            .enumerate()
            .map(|(i, scale)| {
                let world = Mat4::from_scale_rotation_translation(
                    Vec3::splat(scale),
                    glam::Quat::IDENTITY,
                    Vec3::new(i as f32 * 2. - 1., 0., -10.),
                );
                instanced::Instance {
                    entity: i as u64 + 1,
                    name: format!("instance-{i}"),
                    geometry: 0,
                    material: 0,
                    world,
                    previous_world: world,
                }
            })
            .collect(),
        materials: vec![instanced::Material::default()],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ONE,
        environment: vec![[0.02, 0.03, 0.04, 1.]],
        environment_size: [1, 1],
    }
}

fn triangles(renderer: &Renderer, pass: usize) -> u32 {
    renderer.native.as_ref().unwrap().culling.draws[pass]
        .iter()
        .map(|batch| batch.count * (batch.vertices / 3))
        .sum()
}

#[test]
fn raster_quality_controls_native_lod_and_small_instance_culling_without_idle_work() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/cull-budgets")
        .join(std::process::id().to_string());
    let mut scene = scene();
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [128, 128],
        &[],
        root.join("cache"),
    )?;
    let camera = CameraSpec::LookAt {
        position: Vec3::ZERO,
        target: -Vec3::Z,
        vertical_fov: 1.,
    };
    let mut quality = *renderer.quality();
    quality.min_pixels = 0.;
    quality.lod_pixels = 1.;
    renderer.set_quality(quality)?;
    renderer.render(&camera, 1.)?;
    assert_eq!(
        triangles(&renderer, 4),
        3,
        "fine visible mesh plus tiny coarse mesh"
    );
    let request = CaptureRequest {
        camera,
        size: [128, 128],
        mode: ViewMode::EntityIds,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let before = renderer.capture(&request)?;
    before.write(&root.join("before.png"))?;
    quality.min_pixels = 1.5;
    quality.lod_pixels = 2.;
    renderer.set_quality(quality)?;
    renderer.render(&camera, 1.)?;
    assert_eq!(
        triangles(&renderer, 4),
        1,
        "only the visible coarser mesh remains"
    );
    let after = renderer.capture(&request)?;
    after.write(&root.join("after.png"))?;
    for y in 0..128 {
        for x in 0..128 {
            assert_eq!(
                before.entity_at(x, y) == Some(1),
                after.entity_at(x, y) == Some(1),
                "visible silhouette changed at {x},{y}"
            );
        }
    }
    renderer.render(&camera, 1.)?;
    let work = renderer.native_frame_work().unwrap();
    assert_eq!(
        (work.lod_tests, work.cull_tests, work.visible_rows),
        (0, 0, 0)
    );
    // Equal aspect ratio keeps the projection matrix unchanged: the pixel
    // extent itself must invalidate the LOD and visible-list cache.
    renderer.resize([256, 256])?;
    renderer.render(&camera, 1.)?;
    assert_eq!(triangles(&renderer, 4), 2);
    renderer.resize([128, 128])?;
    quality.lod_pixels = 1.;
    quality.min_pixels = 0.;
    renderer.set_quality(quality)?;
    renderer.render(&camera, 1.)?;
    assert_eq!(triangles(&renderer, 4), 3);

    renderer.reserve_native_shadow_passes(1)?;
    let shadow = Mat4::orthographic_rh(-50., 50., -50., 50., 100., 0.05);
    let pass = renderer.cull_native_shadow_pass_with_resolution(0, shadow, 512)?;
    assert_eq!(
        triangles(&renderer, pass),
        1,
        "shadow chooses its own LOD and drops the tiny caster"
    );
    renderer.cull_native_shadow_pass_with_resolution(0, shadow, 128)?;
    assert_eq!(
        triangles(&renderer, pass),
        0,
        "sub-two-texel casters are omitted"
    );
    renderer.cull_native_shadow_pass_with_resolution(0, shadow, 2048)?;
    assert_eq!(
        triangles(&renderer, pass),
        2,
        "resolution alone invalidates the cached list"
    );
    let before = renderer.native_frame_work().unwrap();
    renderer.cull_native_shadow_pass_with_resolution(0, shadow, 2048)?;
    let after = renderer.native_frame_work().unwrap();
    assert_eq!(
        (before.cull_tests, before.visible_rows),
        (after.cull_tests, after.visible_rows)
    );
    let perspective = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1., 100., 0.05);
    renderer.cull_native_shadow_pass_with_resolution(0, perspective, 32)?;
    assert_eq!(triangles(&renderer, pass), 0);
    renderer.cull_native_shadow_pass_with_resolution(0, perspective, 512)?;
    assert_eq!(triangles(&renderer, pass), 2);
    renderer.cull_native_shadow_with_resolution(shadow, 128)?;
    assert_eq!(triangles(&renderer, 6), 0);
    renderer.cull_native_shadow_with_resolution(shadow, 512)?;
    assert_eq!(triangles(&renderer, 6), 1);

    // Camera LOD is independent of a shadow face's texel LOD. Moving the
    // camera must not invalidate an unchanged local-light face or its casters.
    renderer.render_view(&camera, 1., ViewMode::Albedo)?;
    renderer.cull_native_shadow_pass_with_resolution(0, shadow, 2048)?;
    let near_triangles = triangles(&renderer, 4);
    let distant_camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 60.),
        target: Vec3::new(0., 0., -10.),
        vertical_fov: 1.,
    };
    renderer.render_view(&distant_camera, 1., ViewMode::Albedo)?;
    assert!(triangles(&renderer, 4) < near_triangles);
    let casters = renderer.native_caster_changes().unwrap();
    assert!(!casters.all && casters.bounds.is_empty());
    let before = renderer.native_frame_work().unwrap();
    renderer.cull_native_shadow_pass_with_resolution(0, shadow, 2048)?;
    let after = renderer.native_frame_work().unwrap();
    assert_eq!(
        (before.cull_tests, before.visible_rows),
        (after.cull_tests, after.visible_rows)
    );
    renderer.cull_native_shadow_pass_with_resolution(0, shadow, 128)?;
    assert_eq!(triangles(&renderer, pass), 0);
    scene.instances[0].world *= Mat4::from_scale(Vec3::splat(8.));
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            instances: vec![0],
            ..Default::default()
        },
    )?;
    renderer.render_view(&distant_camera, 1., ViewMode::Albedo)?;
    assert!(!renderer.native_caster_changes().unwrap().bounds.is_empty());
    renderer.cull_native_shadow_pass_with_resolution(0, shadow, 128)?;
    assert_eq!(
        triangles(&renderer, pass),
        2,
        "a changed scale refreshes the cached radius and LOD error scale"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
