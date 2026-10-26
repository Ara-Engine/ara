use super::*;
use crate::{
    capture::ViewMode,
    look::{Quality, Shadows},
};

fn renderer(name: &str) -> Result<Renderer> {
    let mut renderer = Renderer::new_raster(
        Scene {
            meshes: Vec::new(),
            materials: vec![crate::scene::Material {
                textures: [0; 4],
                ..Default::default()
            }],
            textures: vec![crate::scene::Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: false,
            }],
            lights: Vec::new(),
            sun_direction: Vec3::Y,
            sun_lux: Vec3::ZERO,
            environment: vec![[0.2, 0.3, 0.4, 1.]; 8],
            environment_size: [4, 2],
        },
        [48, 32],
        &[],
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-render/probe-preparation")
            .join(name),
    )?;
    renderer.set_quality(Quality {
        gi_budget: 2,
        shadows: Shadows {
            cascades: 1,
            resolution: 256,
            distance: 10.,
            far: None,
        },
        ..Default::default()
    })?;
    Ok(renderer)
}

fn camera() -> CameraSpec {
    CameraSpec::LookAt {
        position: Vec3::new(0., 1., 3.),
        target: Vec3::ZERO,
        vertical_fov: 1.,
    }
}

fn layout() -> ProbeLayout {
    ProbeLayout {
        origin: [-1.; 3],
        spacing: [2.; 3],
        counts: [2; 3],
    }
}

fn update() -> Result<ProbeUpdate> {
    let mut statics = vec![[1., 1., 0., 0.]; 8 * crate::look::gi::STATIC_VECTORS];
    for p in 0..8 {
        statics[p * crate::look::gi::STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    ProbeUpdate::new(
        &ProbeSurfaces {
            origin: layout().origin,
            spacing: layout().spacing,
            counts: layout().counts,
            texels: vec![[255 << 24, 0, 0, 0]; 8 * SURFACE_TEXELS],
            statics,
        },
        None,
    )
}

#[test]
fn initial_probe_resources_clear_in_slices_and_delivery_allocates_no_descriptors_or_waits()
-> Result<()> {
    let mut renderer = renderer("slices")?;
    assert!(!renderer.preallocate_probe_volume(layout(), 4)?);
    let usage = renderer.gpu.descriptor_usage();
    let waits = renderer.gpu.context().host_wait_stats().calls;
    for _ in 0..4 {
        assert!(!renderer.preallocate_probe_volume(layout(), 4)?);
    }
    assert_eq!(renderer.gpu.context().host_wait_stats().calls, waits);
    assert_eq!(renderer.gpu.descriptor_usage(), usage);
    for cursor in [4, 8] {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        let gi = renderer.look.gi.as_ref().context("reserved volume")?;
        assert_eq!(gi.clear_cursor, cursor);
        assert!(renderer.timings().iter().any(|p| p.pass == "GI.Clear"));
        assert!(!renderer.timings().iter().any(|p| p.pass == "GI.Relight"));
    }
    assert!(!renderer.preallocate_probe_volume(layout(), 4)?);
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    assert!(renderer.preallocate_probe_volume(layout(), 4)?);
    // Gather targets are lazily allocated by the loading renders as well.
    let usage = renderer.gpu.descriptor_usage();
    let waits = renderer.gpu.context().host_wait_stats().calls;
    let update = update()?;
    renderer.update_probe_records(&update, 0..4)?;
    assert_eq!(renderer.gpu.context().host_wait_stats().calls, waits);
    assert_eq!(renderer.gpu.descriptor_usage(), usage);
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    let waits = renderer.gpu.context().host_wait_stats().calls;
    renderer.update_probe_records(&update, 4..8)?;
    assert_eq!(renderer.gpu.context().host_wait_stats().calls, waits);
    assert_eq!(renderer.gpu.descriptor_usage(), usage);
    for frame in 0..8 {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        let gi = renderer.look.gi.as_ref().context("delivered volume")?;
        if frame < 7 {
            assert!(gi.converge > 0, "live work stays budgeted");
        }
    }
    assert_eq!(
        renderer
            .look
            .gi
            .as_ref()
            .context("converged volume")?
            .converge,
        0
    );
    renderer.reset_capture_history()?;
    for _ in 0..8 {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    assert_eq!(
        renderer
            .look
            .gi
            .as_ref()
            .context("capture volume")?
            .converge,
        0
    );
    renderer.gpu.assert_clean()?;
    Ok(())
}

#[test]
fn initial_bounces_use_a_bounded_window_then_restore_the_preset_budget() -> Result<()> {
    let mut renderer = renderer("initial-budget")?;
    let mut quality = *renderer.quality();
    quality.gi_budget = 256;
    renderer.set_quality(quality)?;
    let counts = [32, 16, 16];
    let probes = 8192;
    let mut statics = vec![[1., 1., 0., 0.]; probes * crate::look::gi::STATIC_VECTORS];
    for p in 0..probes {
        statics[p * crate::look::gi::STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    renderer.set_probe_surfaces(Some(&ProbeSurfaces {
        origin: [-1.; 3],
        spacing: [2.; 3],
        counts,
        texels: vec![[255 << 24, 0, 0, 0]; probes * SURFACE_TEXELS],
        statics,
    }))?;
    for frame in 1..=16 {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        let gi = renderer.look.gi.as_ref().context("GI volume")?;
        assert_eq!(gi.converge, 8 - frame / 2);
        assert_eq!(gi.converge_progress, (frame % 2) * 4096);
    }
    assert!(
        !renderer.probe_gi_ready(),
        "readiness still requires the visibility fade"
    );
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    assert_eq!(
        renderer.look.gi.as_ref().context("GI volume")?.cursor,
        256,
        "steady relight returns to the preset budget"
    );
    for _ in 0..30 {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    assert!(renderer.probe_gi_ready());
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn explicit_capture_can_finish_a_partial_probe_clear_before_delivering_records() -> Result<()> {
    let mut renderer = renderer("capture")?;
    assert!(!renderer.preallocate_probe_volume(layout(), 4)?);
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    assert!(renderer.probe_preparation_pending());
    renderer.complete_probe_preparation()?;
    assert!(!renderer.probe_preparation_pending());
    renderer.update_probe_surfaces(&update()?)?;
    renderer.reset_capture_history()?;
    for _ in 0..8 {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    assert_eq!(
        renderer
            .look
            .gi
            .as_ref()
            .context("capture volume")?
            .received,
        8
    );
    renderer.gpu.assert_clean()?;
    Ok(())
}
