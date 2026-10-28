use super::*;
use crate::QualityPreset;
use crate::probes::{ProbeFilter, ProbeKey, ProbeRow, ProbeState, RecaptureRequest};
use ara_components::render::ReflectionProbe;
use ara_runtime::extract::ProbeView;

const DOC: &str = "@levels/main.scene";
const PROBE: &str = "e_probe";

fn scene(intensity: f32) -> RenderFrame {
    let mut frame = RenderFrame::default();
    frame
        .draws
        .push(box_draw(1, [4., 0.2, 4.], [0., -0.1, 0.], [0.; 3]));
    frame
        .draws
        .push(box_draw(2, [1.; 3], [1.5, 0.5, 0.], [0.; 3]));
    frame.reflection_probes.push(ProbeView {
        address: PROBE.into(),
        entity: 7,
        probe: ReflectionProbe {
            capture_resolution: 64,
            intensity,
            ..Default::default()
        },
        translation: [0., 1., 0.],
        rotation: [0., 0., 0., 1.],
        scale: [1.; 3],
    });
    frame
}

/// One editor frame: prepare the retained frame (probe jobs tick there), then draw.
fn step(
    raster: &mut RasterView,
    renderer: &mut Renderer,
    frame: &RenderFrame,
    plan: RasterPlan,
) -> Result<()> {
    raster.prepare_unchanged(renderer, frame, plan, crate::environment(frame), || {
        Ok(Content::default())
    })?;
    let camera = CameraSpec::LookAt {
        position: glam30::Vec3::new(0., 2., 5.),
        target: glam30::Vec3::ZERO,
        vertical_fov: 55f32.to_radians(),
    };
    raster.render(renderer, frame, &camera, ViewMode::Lit)?;
    Ok(())
}

fn row(raster: &RasterView) -> ProbeRow {
    let rows = raster
        .probes()
        .snapshot(&ProbeFilter::default(), None)
        .probes;
    rows.into_iter()
        .find(|r| r.entity == PROBE)
        .expect("probe row")
}

/// Steps until `done` holds, recording each distinct row state on the way. Bounded
/// by wall time: a cold pipeline cache makes the first frames slow.
fn run_until(
    raster: &mut RasterView,
    renderer: &mut Renderer,
    frame: &RenderFrame,
    plan: RasterPlan,
    mut done: impl FnMut(&RasterView) -> bool,
) -> Result<Vec<ProbeState>> {
    let mut states = Vec::new();
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(120) {
        step(raster, renderer, frame, plan)?;
        let state = row(raster).state;
        if states.last() != Some(&state) {
            states.push(state);
        }
        if done(raster) {
            return Ok(states);
        }
        // Readbacks and cache writes complete off this thread.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let last = row(raster);
    anyhow::bail!(
        "probe work did not finish: {states:?}; stage {:?}, bake {:?}",
        last.stage,
        renderer.reflection_capture_status(7)
    )
}

/// A fresh per-process test folder under `target/runtime-render/<name>`.
fn folder(name: &str) -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render")
        .join(name)
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn plan() -> RasterPlan {
    RasterPlan {
        render_size: [96, 64],
        display_size: [96, 64],
        quality: QualityPreset::High.raster_quality(),
    }
}

/// Opens `frame` at High on a fresh device, its probes bound to `root`'s cache.
fn open(root: &std::path::Path, frame: RenderFrame) -> Result<(Renderer, RasterView, RenderFrame)> {
    let candidate = RasterContent::new(frame, Content::default())?;
    let mut renderer = Renderer::new_native(
        candidate.native.scene(),
        Box::new(candidate.prepared.textures.fork_sources(texture_config())?),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    let mut raster = RasterView::new(root.join("gi"));
    raster.set_probe_gi(false);
    raster
        .probes_mut()
        .set_scene(DOC, root.join("project/.ara/cache/probes"));
    let frame = raster.publish_scene(&mut renderer, candidate, plan())?;
    Ok((renderer, raster, frame))
}

/// Waits (bounded) for a detached cache write to land.
fn wait_for_file(path: &std::path::Path) -> Result<()> {
    let started = std::time::Instant::now();
    while !path.is_file() {
        anyhow::ensure!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "{} was never written",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(())
}

#[test]
fn probe_recapture_publishes_caches_cancels_and_restores() -> Result<()> {
    let root = folder("probe-jobs");
    let cache = root.join("project/.ara/cache/probes");
    let plan = plan();
    let (mut renderer, mut raster, frame) = open(&root, scene(1.))?;
    step(&mut raster, &mut renderer, &frame, plan)?;
    let snapshot = raster.probes().snapshot(&ProbeFilter::default(), None);
    assert!(
        snapshot.capabilities.reflection_probes,
        "{:?}",
        snapshot.capabilities
    );
    let idle = row(&raster);
    assert_eq!(
        idle.state,
        ProbeState::Idle,
        "explicit probes wait for a request"
    );
    assert!(!idle.source_hash.is_empty());

    // Request: queued, capturing/progress, then a published receipt and cache file.
    let key = ProbeKey {
        doc: DOC.into(),
        entity: PROBE.into(),
    };
    let request = RecaptureRequest {
        probes: std::slice::from_ref(&key),
        why: Some("test"),
    };
    let job = raster
        .probes_mut()
        .recapture(&frame, &request)
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let states = run_until(&mut raster, &mut renderer, &frame, plan, |r| {
        r.probes().job(job).is_some_and(|j| j.finished)
    })?;
    let status = raster.probes().job(job).expect("job");
    assert!(status.failures.is_empty(), "{:?}", status.failures);
    let receipt = status.receipts.first().expect("receipt").clone();
    assert_eq!(
        (receipt.faces, receipt.resolution, receipt.generation),
        (6, 64, 1)
    );
    assert_eq!(receipt.source_hash, idle.source_hash);
    assert!(
        states.contains(&ProbeState::Progress) && states.last() == Some(&ProbeState::Done),
        "{states:?}"
    );
    // The cache file follows the receipt, off the job's path.
    wait_for_file(&cache.join(format!("{}.capture", receipt.source_hash)))?;
    assert_eq!(row(&raster).last_receipt.as_ref(), Some(&receipt));

    // Cancel: the request ends Cancelled and the published capture stays.
    let job = raster
        .probes_mut()
        .recapture(&frame, &request)
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    assert!(
        raster
            .probes_mut()
            .cancel(job)
            .expect("job")
            .cancel_requested
    );
    run_until(&mut raster, &mut renderer, &frame, plan, |r| {
        r.probes().job(job).is_some_and(|j| j.finished)
    })?;
    let status = raster.probes().job(job).expect("job");
    assert_eq!(
        status.failures.first().map(|f| f.state),
        Some(ProbeState::Cancelled)
    );
    let cancelled = row(&raster);
    assert_eq!(cancelled.state, ProbeState::Cancelled);
    assert_eq!(cancelled.last_receipt.as_ref(), Some(&receipt));

    // An input edit leaves the published capture stale until recaptured.
    let edited = scene(2.);
    raster.prepare(
        &mut renderer,
        &edited,
        plan,
        crate::environment(&edited),
        || Ok(Content::default()),
    )?;
    let stale = row(&raster);
    assert!(
        stale.stale && stale.source_hash != receipt.source_hash,
        "{stale:?}"
    );
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    drop(renderer);

    // Reopening the scene restores the capture from the disk cache without a request.
    let (mut renderer, mut raster, frame) = open(&root, scene(1.))?;
    run_until(&mut raster, &mut renderer, &frame, plan, |r| {
        row(r).last_receipt.is_some()
    })?;
    let restored = row(&raster);
    assert_eq!(restored.state, ProbeState::Done);
    assert_eq!(
        restored.last_receipt.as_ref().map(|r| &r.capture_hash),
        Some(&receipt.capture_hash)
    );
    assert!(
        renderer
            .reflection_capture_status(7)
            .is_some_and(|s| s.ready)
    );
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn a_capture_whose_image_never_reaches_the_job_is_read_back_again() -> Result<()> {
    let root = folder("probe-reread");
    let plan = plan();
    let (mut renderer, mut raster, frame) = open(&root, scene(1.))?;
    step(&mut raster, &mut renderer, &frame, plan)?;
    let key = ProbeKey {
        doc: DOC.into(),
        entity: PROBE.into(),
    };
    let job = raster
        .probes_mut()
        .recapture(
            &frame,
            &RecaptureRequest {
                probes: std::slice::from_ref(&key),
                why: None,
            },
        )
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    // The first published image is lost before the job sees it: the job reads the
    // column back again instead of staying in Progress.
    let (mut lost, started) = (0, std::time::Instant::now());
    while !raster.probes().job(job).is_some_and(|j| j.finished) {
        anyhow::ensure!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "the job never finished: {:?}",
            row(&raster)
        );
        if lost == 0 {
            lost = renderer.take_reflection_capture_images().len();
        }
        step(&mut raster, &mut renderer, &frame, plan)?;
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let status = raster.probes().job(job).expect("job");
    assert_eq!(lost, 1);
    assert!(status.failures.is_empty(), "{:?}", status.failures);
    assert_eq!(status.receipts.len(), 1);
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn retirement_waits_for_a_cache_write_that_outlives_its_job() -> Result<()> {
    let root = folder("probe-retire");
    let plan = plan();
    let (mut renderer, mut raster, frame) = open(&root, scene(1.))?;
    let gate = std::sync::Arc::new(std::sync::Mutex::new(()));
    let held = gate.lock().expect("write gate");
    raster.probes_mut().write_gate = Some(gate.clone());
    step(&mut raster, &mut renderer, &frame, plan)?;
    let key = ProbeKey {
        doc: DOC.into(),
        entity: PROBE.into(),
    };
    let job = raster
        .probes_mut()
        .recapture(
            &frame,
            &RecaptureRequest {
                probes: std::slice::from_ref(&key),
                why: None,
            },
        )
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    // The job completes on its receipt while its cache write is held open.
    run_until(&mut raster, &mut renderer, &frame, plan, |r| {
        r.probes().job(job).is_some_and(|j| j.finished)
    })?;
    let receipt = raster.probes().job(job).expect("job").receipts[0].clone();
    let file = root
        .join("project/.ara/cache/probes")
        .join(format!("{}.capture", receipt.source_hash));
    assert!(!file.exists());
    assert!(raster.probes().is_busy() && raster.needs_frame());
    for _ in 0..8 {
        assert!(
            raster.poll_probe_retirement(&mut renderer)?,
            "a held write keeps the view"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // Released: retirement drains the write and reports the view free.
    drop(held);
    let started = std::time::Instant::now();
    while raster.poll_probe_retirement(&mut renderer)? {
        anyhow::ensure!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the released write never drained"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(file.is_file() && !raster.probes().is_busy());
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}
