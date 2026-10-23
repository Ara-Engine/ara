//! `capture`: a project scene through the fidelity renderer (ara-render-fidelity:
//! RT hybrid transport, DDGI, ReSTIR, atmosphere, auto exposure, TAA/FSR1). The
//! engine has no project-scene → fidelity path outside the editor viewport (which
//! draws imported models untextured), so this module bridges Play's extraction to a
//! fidelity snapshot: the scene is cooked and simulated exactly as Play does it.
mod atlas;
pub mod convert;
pub mod look;
pub mod render;
mod vfx;
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
    /// Only the first shot, native only, no timings worth keeping.
    pub quick: bool,
    /// Timing frames per mode.
    pub timing_frames: u32,
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
    let worker = tool("ara-import-worker", "ARA_IMPORT_WORKER")?;
    let started = std::time::Instant::now();
    let mut world = world::load(&root, &look.scene, &worker, look.ticks)?;
    let frame = world.frame();
    if look.ground_relative
        && let Some(view) = &frame.terrain
    {
        let ground = |p: [f32; 3]| view.data.height(p[0], p[2]).unwrap_or(0.0);
        for shot in &mut look.shots {
            shot.position[1] += ground(shot.position);
            shot.target[1] += ground(shot.target);
        }
    }
    let (scene, stats) = convert::scene(&mut world, &frame, &look, lock, &root)?;
    eprintln!(
        "{}: {} objects -> {} meshes, {} triangles ({} at finest LOD), {} foliage instances, {} textures ({} source maps, {} materials atlased), {} lights, ~{:.0} MiB upload ({:.1} s to load)",
        request.look,
        stats.objects,
        stats.meshes,
        stats.triangles,
        stats.finest_triangles,
        stats.foliage_instances,
        stats.textures,
        stats.textures_wanted,
        stats.atlased_materials,
        stats.lights,
        stats.upload_mib,
        started.elapsed().as_secs_f64()
    );
    for (name, triangles) in &stats.breakdown {
        eprintln!("  {triangles:>9} triangles  {name}");
    }
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-samples/pipeline-cache");
    std::fs::create_dir_all(&cache)?;
    let mut gpu = ara_gpu::vulkan::VulkanDevice::new(&[], cache)?;
    let evidence = root.join("evidence");
    let scratch = root.join(".scratch/captures");
    render::run(
        &mut gpu,
        &scene,
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
            effects: &frame.effects,
        },
    )?;
    gpu.assert_clean()?;
    Ok(())
}
