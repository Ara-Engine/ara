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
    ProbeLayout::single(crate::look::gi::ProbeLattice {
        origin: [-1.; 3],
        spacing: [2.; 3],
        counts: [2; 3],
    })
}

fn update() -> Result<ProbeUpdate> {
    let mut statics = vec![[1., 1., 0., 0.]; 8 * crate::look::gi::STATIC_VECTORS];
    for p in 0..8 {
        statics[p * crate::look::gi::STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    ProbeUpdate::new(
        &ProbeSurfaces {
            layout: layout(),
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
    let waits = renderer.gpu.host_wait_stats().calls;
    for _ in 0..4 {
        assert!(!renderer.preallocate_probe_volume(layout(), 4)?);
    }
    assert_eq!(renderer.gpu.host_wait_stats().calls, waits);
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
    let waits = renderer.gpu.host_wait_stats().calls;
    let update = update()?;
    renderer.update_probe_records(&update, 0..4)?;
    assert_eq!(renderer.gpu.host_wait_stats().calls, waits);
    assert_eq!(renderer.gpu.descriptor_usage(), usage);
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    let waits = renderer.gpu.host_wait_stats().calls;
    renderer.update_probe_records(&update, 4..8)?;
    assert_eq!(renderer.gpu.host_wait_stats().calls, waits);
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
        layout: ProbeLayout::single(crate::look::gi::ProbeLattice {
            origin: [-1.; 3],
            spacing: [2.; 3],
            counts,
        }),
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
fn unseen_frames_converge_whole_bounces_and_show_the_volume_without_a_fade() -> Result<()> {
    let mut renderer = renderer("unseen")?;
    let mut quality = *renderer.quality();
    quality.gi_budget = 256;
    renderer.set_quality(quality)?;
    renderer.set_unseen(true);
    let probes = 8192;
    let mut statics = vec![[1., 1., 0., 0.]; probes * crate::look::gi::STATIC_VECTORS];
    for p in 0..probes {
        statics[p * crate::look::gi::STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    renderer.set_probe_surfaces(Some(&ProbeSurfaces {
        layout: ProbeLayout::single(crate::look::gi::ProbeLattice {
            origin: [-1.; 3],
            spacing: [2.; 3],
            counts: [32, 16, 16],
        }),
        texels: vec![[255 << 24, 0, 0, 0]; probes * SURFACE_TEXELS],
        statics,
    }))?;
    // One whole bounce per frame (a seen volume takes two, then a 30-frame fade).
    for frame in 1..=8 {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        assert_eq!(
            renderer.look.gi.as_ref().context("GI volume")?.converge,
            8 - frame
        );
    }
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    assert!(renderer.probe_gi_ready(), "no fade while unseen");
    renderer.set_unseen(false);
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    assert_eq!(
        renderer.look.gi.as_ref().context("GI volume")?.cursor,
        512,
        "shown frames relight at the preset budget"
    );
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

/// Eight fine probes inside twelve coarse ones (`ProbeLayout`, finest first).
fn pool() -> Result<ProbeLayout> {
    ProbeLayout::new(&[
        crate::look::gi::ProbeLattice {
            origin: [-1.; 3],
            spacing: [2.; 3],
            counts: [2; 3],
        },
        crate::look::gi::ProbeLattice {
            origin: [-4., -4., -4.],
            spacing: [4.; 3],
            counts: [3, 2, 2],
        },
    ])
}

#[test]
fn a_two_lattice_pool_reserves_before_delivery_and_accepts_packets_across_its_boundary()
-> Result<()> {
    let mut renderer = renderer("pool")?;
    let layout = pool()?;
    assert!(!renderer.preallocate_probe_volume(layout, 6)?);
    let usage = renderer.gpu.descriptor_usage();
    // The whole pool clears in bounded slices behind loading; polling the
    // reservation allocates and waits for nothing more.
    loop {
        let waits = renderer.gpu.host_wait_stats().calls;
        let ready = renderer.preallocate_probe_volume(layout, 6)?;
        assert_eq!(renderer.gpu.host_wait_stats().calls, waits);
        assert_eq!(
            renderer.gpu.descriptor_usage().storage_buffers,
            usage.storage_buffers
        );
        if ready {
            break;
        }
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    // Loading renders also allocate the gather targets, before any delivery.
    let usage_after_clear = renderer.gpu.descriptor_usage();
    let mut statics = vec![[1., 1., 0., 0.]; 20 * crate::look::gi::STATIC_VECTORS];
    for p in 0..20 {
        statics[p * crate::look::gi::STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    let update = ProbeUpdate::new(
        &ProbeSurfaces {
            layout,
            texels: vec![[255 << 24, 0, 0, 0]; 20 * SURFACE_TEXELS],
            statics,
        },
        None,
    )?;
    // Packets (at most the reserved six records) need not align with the lattices:
    // the second spans probe 8, where the coarse lattice begins. Delivery
    // allocates and waits for nothing.
    for packet in [0..5, 5..11, 11..17, 17..20] {
        let waits = renderer.gpu.host_wait_stats().calls;
        renderer.update_probe_records(&update, packet)?;
        assert_eq!(renderer.gpu.host_wait_stats().calls, waits);
        assert_eq!(renderer.gpu.descriptor_usage(), usage_after_clear);
        assert!(!renderer.probe_gi_ready());
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    let gi = renderer.look.gi.as_ref().context("delivered pool")?;
    assert_eq!(
        (
            gi.received,
            gi.deferred_received,
            gi.layout.lattices().len()
        ),
        (12, 8, 2)
    );
    // Ready once the coarse lattice's records landed and it converged and faded
    // in; the fine lattice follows it.
    let mut frames = 0;
    while !renderer.probe_gi_ready() {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        frames += 1;
        ensure!(frames < 256, "the pool never became ready");
    }
    assert_eq!(renderer.look.gi.as_ref().context("ready pool")?.converge, 0);
    renderer.gpu.assert_clean()?;
    Ok(())
}

#[test]
fn a_pool_is_ready_on_its_coarse_lattice_and_cross_fades_the_fine_one_in_later() -> Result<()> {
    let mut renderer = renderer("staged")?;
    let layout = pool()?;
    while !renderer.preallocate_probe_volume(layout, 6)? {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    let mut statics = vec![[1., 1., 0., 0.]; 20 * crate::look::gi::STATIC_VECTORS];
    for p in 0..20 {
        statics[p * crate::look::gi::STATIC_VECTORS + 64] = [0., 0., 0., 1.];
    }
    let surfaces = ProbeSurfaces {
        layout,
        texels: vec![[255 << 24, 0, 0, 0]; 20 * SURFACE_TEXELS],
        statics,
    };
    // The coarse lattice (probes 8..20) lands first; the volume is ready on it.
    let coarse = ProbeUpdate::new(&surfaces, Some(&(8..20).collect::<Vec<_>>()))?;
    let fine = ProbeUpdate::new(&surfaces, Some(&(0..8).collect::<Vec<_>>()))?;
    assert!(fine.finer_only() && !coarse.finer_only());
    for packet in [0..6, 6..12] {
        renderer.update_probe_records(&coarse, packet)?;
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    let mut frames = 0;
    while !renderer.probe_gi_ready() {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        frames += 1;
        ensure!(frames < 256, "the coarse lattice never became ready");
    }
    let gi = renderer.look.gi.as_ref().context("ready pool")?;
    assert_eq!(gi.deferred_weight, 0.);
    // The fine lattice lands later: readiness holds while it converges and
    // cross-fades in, a frame's step at a time.
    for packet in [0..6, 6..8] {
        renderer.update_probe_records(&fine, packet)?;
        assert!(renderer.probe_gi_ready());
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    let mut weight = 0.;
    while weight < 1. {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        let next = renderer.look.gi.as_ref().context("pool")?.deferred_weight;
        assert!(renderer.probe_gi_ready());
        assert_eq!(renderer.probe_gi_needs_frame(), next < 1.);
        assert!(
            (0. ..=1. / 29.).contains(&(next - weight)),
            "{weight} -> {next}"
        );
        weight = next;
        frames += 1;
        ensure!(frames < 512, "the fine lattice never faded in");
    }
    renderer.gpu.assert_clean()?;
    Ok(())
}

/// Both irradiance atlases, probe by probe: the 8x8 bordered tile's RGB texels.
fn atlases(renderer: &mut Renderer) -> Result<[Vec<[f32; 3]>; 2]> {
    let gi = renderer.look.gi.as_mut().context("probe volume")?;
    let (width, height) = (gi.atlas * 8, gi.probes.div_ceil(gi.atlas) * 8);
    let (images, probes, atlas) = (gi.irradiance, gi.probes, gi.atlas);
    let mut buffers = Vec::new();
    for _ in images {
        buffers.push(renderer.gpu.create_buffer(
            u64::from(width * height * 8),
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?);
    }
    let mut commands = renderer.gpu.begin(Queue::Graphics);
    for ((image, access), &buffer) in images.into_iter().zip(&mut gi.access).zip(&buffers) {
        let barrier = |resource, before, after| ara_gpu::Barrier {
            resource,
            before,
            after,
            handoff: None,
        };
        commands.barriers([
            barrier(Resource::Image(image), *access, Access::CopySource),
            barrier(
                Resource::Buffer(buffer),
                Access::HostRead,
                Access::CopyDestination,
            ),
        ]);
        commands.push(Command::ReadImage {
            source: image,
            destination: buffer,
        });
        commands.barriers([barrier(
            Resource::Buffer(buffer),
            Access::CopyDestination,
            Access::HostRead,
        )]);
        *access = Access::CopySource;
    }
    let point = renderer.gpu.submit(commands.finish(), &[])?;
    renderer.gpu.resolve(point)?;
    let half = |bits: u16| {
        let (exponent, mantissa) = (i32::from((bits >> 10) & 31), f32::from(bits & 1023));
        let magnitude = if exponent == 0 {
            mantissa * 2f32.powi(-24)
        } else {
            (1. + mantissa / 1024.) * 2f32.powi(exponent - 15)
        };
        if bits >> 15 == 1 {
            -magnitude
        } else {
            magnitude
        }
    };
    let mut tiles = [Vec::new(), Vec::new()];
    for (tiles, buffer) in tiles.iter_mut().zip(buffers) {
        let bytes = renderer.gpu.read(buffer)?;
        renderer.gpu.retire(Resource::Buffer(buffer))?;
        for id in 0..probes {
            for texel in 0..64 {
                let x = id % atlas * 8 + texel % 8;
                let y = id / atlas * 8 + texel / 8;
                let at = ((y * width + x) * 8) as usize;
                let channel = |c: usize| {
                    half(u16::from_le_bytes([
                        bytes[at + c * 2],
                        bytes[at + c * 2 + 1],
                    ]))
                };
                tiles.push([channel(0), channel(1), channel(2)]);
            }
        }
    }
    Ok(tiles)
}

#[test]
fn a_budgeted_relight_copies_only_the_last_window_forward_and_keeps_both_atlases_current()
-> Result<()> {
    let mut renderer = renderer("relight-window")?;
    let statics = || {
        let mut statics = vec![[1., 1., 0., 0.]; 8 * crate::look::gi::STATIC_VECTORS];
        for p in 0..8 {
            statics[p * crate::look::gi::STATIC_VECTORS + 64] = [0., 0., 0., 1.];
        }
        statics
    };
    renderer.set_probe_surfaces(Some(&ProbeSurfaces {
        layout: layout(),
        texels: vec![[255 << 24, 0, 0, 0]; 8 * SURFACE_TEXELS],
        statics: statics(),
    }))?;
    let mut frames = 0;
    while !renderer.probe_gi_ready() {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
        frames += 1;
        ensure!(frames < 256, "the sky-lit volume never became ready");
    }
    // Every texel now sees an emitter (no sky, black albedo): each probe relights
    // to pi times its radiance once its window comes round, two probes a frame.
    let emissive = [2., 1., 0.5];
    let texel = [0, 0, 0x3c00_4000, 0x3c00_3800];
    let update = ProbeUpdate::new(
        &ProbeSurfaces {
            layout: layout(),
            texels: vec![texel; 8 * SURFACE_TEXELS],
            statics: statics(),
        },
        None,
    )?;
    renderer.update_probe_records(&update, 0..8)?;
    for _ in 0..96 {
        renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    }
    let gi = renderer.look.gi.as_ref().context("relit volume")?;
    assert_eq!(gi.relight_groups(2), 4, "relight work follows the budget");
    // The windows alternate between the atlases: without the copy forward, half
    // the probes would keep the sky in one of them.
    let expected = emissive.map(|e: f32| e * std::f32::consts::PI);
    for (slot, tiles) in atlases(&mut renderer)?.iter().enumerate() {
        for (i, rgb) in tiles.iter().enumerate() {
            for (value, expected) in rgb.iter().zip(expected) {
                assert!(
                    (value - expected).abs() < expected * 0.01,
                    "atlas {slot} probe {} texel {}: {rgb:?}, expected {expected:?}",
                    i / 64,
                    i % 64
                );
            }
        }
    }
    renderer.render_view(&camera(), 1., ViewMode::Lit)?;
    renderer.gpu.assert_clean()?;
    Ok(())
}
