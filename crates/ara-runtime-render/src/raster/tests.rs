use super::*;
use ara_runtime::{
    components::MeshShape,
    extract::{Draw, Geometry},
};
mod preparation;

#[test]
fn disabled_probe_gi_does_not_report_a_queued_emitter_search() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/disabled-probes");
    let mut presenter = crate::Presenter::new([64, 48], Vec::new(), root.join("gpu"));
    let mut raster = RasterView::new(root.join("probes"));
    let prepared = native::prepare(
        &RenderFrame::default(),
        Content::default(),
        texture_config(),
    )?;
    raster
        .emitter_search
        .request(|| crate::emitters::Job::new(&prepared.scene, &[], |_, _, _| None))?;
    // The rerun cannot finish before poll adopts the first result, regardless of
    // worker scheduling. This forces the load-dependent prewarm failure.
    raster.emitter_search.request(|| unreachable!("queued"))?;
    raster.set_probe_gi(false);
    assert!(
        raster.needs_frame(),
        "emitter work still needs presentation"
    );
    presenter.raster = Some(raster);
    assert!(
        !presenter.probe_gi_pending(),
        "disabling GI must not count unrelated emitter work"
    );
    let raster = presenter.raster.as_mut().expect("raster");
    raster.set_probe_gi(true);
    assert!(
        presenter.probe_gi_pending(),
        "new GI acquisition is pending"
    );
    presenter
        .raster
        .as_mut()
        .expect("raster")
        .set_probe_gi(false);
    assert!(!presenter.probe_gi_pending());
    Ok(())
}

fn box_draw(id: u64, size: [f32; 3], position: [f32; 3], emission: [f32; 3]) -> Draw {
    Draw {
        entity: id,
        address: format!("e_{id}"),
        label: format!("Box {id}"),
        geometry: Geometry::Shape {
            shape: MeshShape::Box,
            size,
        },
        world: glam34::Mat4::from_translation(glam34::Vec3::from_array(position)),
        color: [0.7, 0.7, 0.7, 1.],
        roughness: 0.8,
        metallic: 0.,
        emissive: emission,
        emissive_factor: None,
        material: None,
        skin: None,
    }
}

fn terrain_content(color: [f32; 4]) -> Content {
    Content {
        terrain_materials: std::collections::BTreeMap::from([(
            "@ground.graph#gr_ground".into(),
            std::sync::Arc::new(ara_runtime::material::MaterialAsset {
                asset: "gr_ground".into(),
                digest: ara_asset::Digest::of(bytemuck::cast_slice(&color)),
                material: ara_runtime::model::ModelMaterial {
                    base_color: color,
                    ..Default::default()
                },
                textures: Vec::new(),
                texture_generations: Vec::new(),
                base_color_decode_srgb: false,
                emissive_decode_srgb: false,
            }),
        )]),
        ..Default::default()
    }
}

#[test]
fn unchanged_secondary_prepare_keeps_generation_and_starts_no_background_jobs() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/secondary-identity");
    let mut frame = RenderFrame::default();
    frame.draws = vec![box_draw(1, [2.; 3], [0.; 3], [8.; 3])];
    let candidate = RasterContent::new(frame, Content::default())?;
    let mut renderer = Renderer::new_native(
        candidate.native.scene(),
        Box::new(candidate.prepared.textures.fork_sources(texture_config())?),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    let plan = RasterPlan {
        render_size: [96, 64],
        display_size: [96, 64],
        quality: ara_render::look::Quality {
            gi_budget: 1024,
            ..Default::default()
        },
    };
    let mut raster = RasterView::new(root.join("probes"));
    let frame = raster.publish_content(&mut renderer, candidate, plan)?;
    raster.wait_probe_gi(&mut renderer)?;
    let serial = raster.gi.as_ref().expect("GI enabled").request_serial();
    let searches = raster.emitter_search.searches_started();
    assert!(
        serial > 0 && searches > 0,
        "fixture must finish a real GI and emitter job"
    );
    let allocation = raster
        .prepared
        .as_ref()
        .expect("prepared")
        .scene
        .geometry
        .as_ptr();
    let key = raster.key;
    let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
    let camera = CameraSpec::LookAt {
        position: glam30::Vec3::new(0., 1., 4.),
        target: glam30::Vec3::ZERO,
        vertical_fov: 55f32.to_radians(),
    };
    let request = ara_render::ViewRequest {
        camera: &camera,
        size: [64, 40],
        render_size: [64, 40],
        quality: plan.quality,
        exposure: 1.,
    };
    let mut view = renderer.create_view()?;
    for _ in 0..16 {
        renderer.submit_secondary(&mut view, &request, |renderer, _| {
            let update = raster.prepare_unchanged(renderer, &frame, plan, look, || {
                panic!("unchanged preview must not resolve content")
            })?;
            assert!(!update.rebuilt);
            Ok(true)
        })?;
        renderer.wait_scene_frame()?;
        assert_eq!(raster.key, key);
        assert_eq!(
            raster
                .prepared
                .as_ref()
                .expect("prepared")
                .scene
                .geometry
                .as_ptr(),
            allocation
        );
        assert_eq!(
            raster.gi.as_ref().expect("GI enabled").request_serial(),
            serial,
            "secondary camera is not a new GI generation"
        );
        assert_eq!(
            raster.emitter_search.searches_started(),
            searches,
            "secondary camera must not search emitters again"
        );
    }
    renderer.release_view(view)?;
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn first_capture_uploads_finished_emitter_search_without_another_prepare() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/emitter-capture")
        .join(std::process::id().to_string());
    let mut frame = RenderFrame::default();
    frame.environment.sky.0 = [0., 0., 0., 1.];
    frame.environment.horizon.0 = [0., 0., 0., 1.];
    frame.environment.ground.0 = [0., 0., 0., 1.];
    frame.environment.exposure = 1.;
    frame.draws = vec![
        box_draw(1, [8., 0.5, 8.], [0., -0.25, 0.], [0.; 3]),
        box_draw(2, [1.; 3], [0., 2., 0.], [100., 80., 60.]),
    ];
    let config = texture_config();
    let mut prepared = native::prepare(&frame, Content::default(), config)?;
    prepared.enable_motion();
    let job = crate::emitters::Job::new(
        &prepared.scene,
        &crate::gi::shading(&prepared),
        |_, _, _| None,
    );
    ensure!(!job.is_empty(), "fixture needs a real emitter search");
    let mut renderer = Renderer::new_native(
        &prepared.scene,
        Box::new(prepared.textures.fork_sources(config)?),
        [160, 90],
        &[],
        root.join("gpu"),
    )?;
    let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
    renderer.set_look(look.raster())?;
    let mut raster = RasterView::new(root.join("probes"));
    raster.set_probe_gi(false);
    raster.look = Some(look);
    raster.prepared = Some(prepared);
    // No prepare runs before the capture: capture itself must adopt the search
    // and upload the lights, rather than relying on worker timing.
    raster.emitter_search.restart(job)?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: glam30::Vec3::new(0., 3., 7.),
            target: glam30::Vec3::new(0., 1., 0.),
            vertical_fov: 50f32.to_radians(),
        },
        size: [160, 90],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 4,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let first = raster.capture(&mut renderer, &frame, &request)?;
    assert!(!raster.emitter_lights.is_empty());
    let plan = ara_render_fidelity::presets::RasterPlan {
        render_size: [160, 90],
        display_size: [160, 90],
        quality: *renderer.quality(),
    };
    raster.prepare(&mut renderer, &frame, plan, look, || Ok(Content::default()))?;
    let after_prepare = raster.capture(&mut renderer, &frame, &request)?;
    first.write(&root.join("first.png"))?;
    after_prepare.write(&root.join("after-prepare.png"))?;
    assert_eq!(first.image, after_prepare.image);
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn cpu_prepared_worker_content_publishes_and_rejection_keeps_last_good() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/prepared-content")
        .join(std::process::id().to_string());
    let coord = ara_terrain::TileCoord { x: 0, z: 0 };
    let terrain = std::sync::Arc::new(ara_runtime::terrain::TerrainData {
        header: ara_terrain::Header {
            origin: [0.; 2],
            spacing: 1.,
            cells: 16,
            min: coord,
            max: ara_terrain::TileCoord { x: 1, z: 1 },
            materials: vec![ara_schema::AssetRef("@ground.graph#gr_ground".into())],
        },
        tiles: std::collections::BTreeMap::from([(
            coord,
            ara_terrain::Tile::flat(16).expect("fixed tile size"),
        )]),
        foliage: Vec::new(),
        foliage_rules: Vec::new(),
    });
    let snapshot = || {
        let mut frame = RenderFrame::default();
        frame.terrain = Some(ara_runtime::extract::TerrainView {
            data: std::sync::Arc::clone(&terrain),
            tiles: vec![coord],
        });
        frame
    };
    let frame = snapshot();
    let worker = std::thread::spawn(move || RasterContent::new(frame, terrain_content([0.7; 4])));
    let candidate = worker.join().expect("CPU content worker")?;
    let mut renderer = Renderer::new_native(
        candidate.native.scene(),
        Box::new(candidate.prepared.textures.fork_sources(texture_config())?),
        [160, 90],
        &[],
        root.join("gpu"),
    )?;
    let mut raster = RasterView::new(root.join("probes"));
    raster.set_probe_gi(false);
    let plan = ara_render_fidelity::presets::RasterPlan {
        render_size: [160, 90],
        display_size: [160, 90],
        quality: *renderer.quality(),
    };
    let frame = raster.publish_content(&mut renderer, candidate, plan)?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: glam30::Vec3::new(8., 7., 22.),
            target: glam30::Vec3::new(8., 0., 8.),
            vertical_fov: 50f32.to_radians(),
        },
        size: [160, 90],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let pending = raster.capture(&mut renderer, &frame, &request)?;
    // A terrain map/material publication changes no field of RenderFrame. The
    // explicit CPU candidate must still replace the old neutral fallback.
    let ready = RasterContent::new(snapshot(), terrain_content([0.1, 0.5, 0.15, 1.]))?;
    let frame = raster.publish_content(&mut renderer, ready, plan)?;
    let first = raster.capture(&mut renderer, &frame, &request)?;
    assert_ne!(
        pending.image, first.image,
        "terrain-only publication must replace its fallback"
    );
    pending.write(&root.join("pending-terrain.png"))?;
    let usage = renderer.gpu().descriptor_usage();
    for _ in 0..4 {
        let mut invalid = RenderFrame::default();
        invalid.draws = vec![box_draw(2, [f32::NAN, 1., 1.], [0.; 3], [0.; 3])];
        assert!(RasterContent::new(invalid, Content::default()).is_err());
        assert_eq!(renderer.gpu().descriptor_usage(), usage);
    }
    let retained = raster.capture(&mut renderer, &frame, &request)?;
    assert_eq!(first.image, retained.image);
    first.write(&root.join("published.png"))?;
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}
