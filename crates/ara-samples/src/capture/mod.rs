//! `capture`: a project scene through each quality preset (Low: the baseline
//! raster; Medium/High/Epic: ara-render-fidelity's RT hybrid transport, DDGI,
//! ReSTIR, atmosphere, auto exposure and TAAU). The engine adapter consumes Play's
//! extraction and the scene's authored environment; samples resolve pinned
//! overrides, frame shots and measure the resulting views. Scenes cook and
//! simulate like Play.
pub mod convert;
pub mod look;
pub mod reference;
pub mod render;
mod study;
pub mod world;

use crate::{build::tool, lock::Lock};
use anyhow::{Context, Result};
use std::path::Path;

/// What `capture` should do.
pub struct Request<'a> {
    /// Project folder under `samples/`.
    pub project: &'a str,
    /// Look name (`look/<name>.look.json`).
    pub look: &'a str,
    /// Only the first shot and no camera-move strip; uses the requested presets.
    pub quick: bool,
    /// Timing frames per mode.
    pub timing_frames: u32,
    /// Quality presets to capture.
    pub presets: &'a [ara_render_fidelity::presets::QualityPreset],
    /// Reference samples per pixel for RMSE/FLIP scoring; zero skips it.
    pub reference_samples: u32,
    /// Medium and above use the opt-in ray-traced renderer.
    pub ray_tracing: bool,
}

/// Runs the captures for one look of one project.
pub fn run(samples: &Path, lock: &Lock, request: &Request<'_>) -> Result<()> {
    let root = samples.join(request.project);
    let path = root
        .join("look")
        .join(format!("{}.look.json", request.look));
    let mut look: look::Look = serde_json::from_str(
        &std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("parsing {}", path.display()))?;
    // Matched four-path evidence uses only authored environment/material/effect
    // settings; look files still supply camera poses and the simulation tick.
    if std::env::var_os("ARA_CAPTURE_AUTHORED_ONLY").is_some() {
        look.sky = None;
        look.post = None;
        look.components = Default::default();
        look.sun_lux = None;
        look.sun_direction = None;
        look.effects.clear();
        look.details.clear();
        look.terrain = None;
    }
    let worker = tool("ara-import-worker", "ARA_IMPORT_WORKER")?;
    let started = std::time::Instant::now();
    let mut world = world::load(&root, &look.scene, &worker, look.ticks)?;
    let mut frame = world.frame();
    if let Some(sun) = frame.sun.as_mut() {
        if let Some(lux) = look.sun_lux {
            sun.1 = lux;
        }
        if let Some(direction) = look.sun_direction {
            sun.0 = glam::Vec3::from_array(direction).normalize().to_array();
        }
    }
    if look.ground_relative
        && let Some(view) = &frame.terrain
    {
        let ground = |p: [f32; 3]| view.data.height(p[0], p[2]).unwrap_or(0.0);
        for shot in &mut look.shots {
            shot.position[1] += ground(shot.position);
            shot.target[1] += ground(shot.target);
        }
    }
    let (scene, stats) = convert::scene(&world, &frame, &look, lock, &root)?;
    // One look for every shot: post-process volumes blend at the first shot's camera
    // (exact for global volumes).
    let camera = look.shots.first().map_or([0.; 3], |shot| shot.position);
    let mut environment = ara_render_fidelity::environment::FrameEnvironment::resolve(
        &look.scene_look(frame.look.clone()),
        glam::Vec3::from_array(camera),
        frame.environment.exposure,
    );
    look.tune_effects(&mut frame.effects);
    let effects = frame.effects.clone();
    look.override_environment(&mut environment);
    eprintln!(
        "{}: {} native instances, {} shared geometries, {} instanced triangles, {} foliage instances, {} compressed texture identities, {} lights ({:.1} s to load)",
        request.look,
        stats.instances,
        stats.geometries,
        stats.finest_instanced_triangles,
        stats.foliage_instances,
        stats.textures,
        stats.lights,
        started.elapsed().as_secs_f64()
    );
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-samples/pipeline-cache");
    std::fs::create_dir_all(&cache)?;
    let mut gpu = ara_gpu::GpuDevice::new(&[], cache.clone())?;
    let output = std::env::var_os("ARA_CAPTURE_OUTPUT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-samples/captures")
        })
        .join(request.project)
        .join(request.look);
    let evidence = output.join("evidence");
    let scratch = output.join("full");
    render::run(
        &mut gpu,
        scene,
        &mut frame,
        &stats,
        &look,
        &render::Options {
            evidence: &evidence,
            scratch: &scratch,
            name: request.look,
            evidence_width: 1280,
            settle: 120,
            timing_frames: request.timing_frames,
            quick: request.quick,
            effects: &effects,
            presets: request.presets,
            environment: &environment,
            pipeline_cache: &cache,
            content: &|frame| {
                let mut content = world.content(frame)?;
                content.environment = convert::environment(&look, lock, &root)?;
                Ok(content)
            },
            reference_samples: request.reference_samples,
            ray_tracing: request.ray_tracing,
        },
    )?;
    gpu.assert_clean()?;
    Ok(())
}
