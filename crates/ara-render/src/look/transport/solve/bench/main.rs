//! Explicit G3 replay/A-B driver. It is an example, never an ordinary test, and
//! writes only to its caller's output directory. Use --features fidelity-lab.
mod fixtures;
mod gltf_scene;
mod images;
mod memory;
use ara_render as fixture_api;

use anyhow::{Context, Result, ensure};
use ara_render::{
    PatchTransport, TransportOptions,
    instanced::transport::{NativeChangeFeed, ReplayWriter},
};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(serde::Serialize)]
struct FrameScore {
    frame: u64,
    uncapped: ara_render::TransportReport,
    capped: ara_render::TransportReport,
    max_error_white: f32,
    mean_error_white: f32,
    region_flip_excess: Option<f64>,
    measured_transport_heap_live: isize,
    measured_transport_heap_peak: usize,
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let value = ara_json::to_value(value)?;
    std::fs::write(
        path,
        ara_json::to_canonical_string(&ara_json::sort_keys(&value)),
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let option = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let out = option("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/transport-g3"));
    std::fs::create_dir_all(&out)?;
    if args.iter().any(|a| a == "--register-pipelines") {
        return register_pipelines();
    }
    if let Some(path) = option("--ab") {
        let fixture = gltf_scene::load(Path::new(path))?;
        return images::ab(
            &fixture,
            &out,
            option("--samples")
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or(1024),
        );
    }
    let frames: Option<u64> = option("--frames").map(|s| s.parse()).transpose()?;
    let selected = option("--case").map(String::as_str);
    let images = args.iter().any(|a| a == "--images");
    let global = args.iter().any(|a| a == "--global");
    let cases = fixtures::cases();
    let mut results = BTreeMap::new();
    for case in cases
        .iter()
        .filter(|case| selected.is_none_or(|name| name == case.name))
    {
        let dir = out.join(case.name);
        std::fs::create_dir_all(&dir)?;
        println!(
            "G3 START {} frames={} directions={}",
            case.name,
            frames.unwrap_or(case.frames),
            if global { "global" } else { "per_patch" }
        );
        std::io::stdout().flush()?;
        let mut fixture = fixtures::create(case)?;
        let options = TransportOptions {
            global_directions: global,
            channel_bytes: if args.iter().any(|a| a == "--channels") {
                256 * 1024 * 1024
            } else {
                0
            },
            ..TransportOptions::default()
        };
        let memory_before = memory::begin();
        let mut uncapped = PatchTransport::new(
            &fixture.scene,
            &fixture.textures,
            fixture
                .bodies
                .iter()
                .map(|(&id, &class)| (id, class))
                .collect(),
            options,
            Some(&out.join("cache")),
        )?;
        let (mut measured_live, cold_peak) = memory::end(memory_before);
        let cold = uncapped.report().clone();
        let mut capped = PatchTransport::new(
            &fixture.scene,
            &fixture.textures,
            fixture
                .bodies
                .iter()
                .map(|(&id, &class)| (id, class))
                .collect(),
            options,
            Some(&out.join("cache")),
        )?;
        let mut feed = NativeChangeFeed::new(&fixture.scene, 0)?;
        let mut initial = ara_render::instanced::Changes::default();
        initial.transport.bodies = fixture
            .bodies
            .iter()
            .map(|(&id, &class)| (id as usize, class))
            .collect();
        feed.update(&fixture.scene, Some(&initial))?;
        let file = std::fs::File::create(dir.join("events.arachg"))?;
        let mut replay = ReplayWriter::new(std::io::BufWriter::new(file))?;
        replay.write_frame(feed.finish())?;
        let mut capture = if images {
            Some(images::ReplayImages::new(&fixture, &dir)?)
        } else {
            None
        };
        let mut scores = Vec::new();
        for frame in 1..frames.unwrap_or(case.frames) {
            let layout = (fixture.scene.geometry.len(), fixture.scene.instances.len());
            let changes = fixtures::advance(&mut fixture, case, frame);
            feed.begin_frame(frame)?;
            if layout != (fixture.scene.geometry.len(), fixture.scene.instances.len()) {
                feed.replace(&fixture.scene)?;
            }
            feed.update(&fixture.scene, Some(&changes))?;
            let events = feed.finish();
            replay.write_frame(events)?;
            let memory_before = memory::begin();
            let demanded = uncapped
                .advance(&fixture.scene, &fixture.textures, events, true)?
                .clone();
            let (delta, extra_peak) = memory::end(memory_before);
            let frame_peak = measured_live.max(0) as usize + extra_peak;
            measured_live += delta;
            let protected = capped
                .advance(&fixture.scene, &fixture.textures, events, false)?
                .clone();
            let exact = uncapped.exact_patch_solution()?;
            let values = capped.patch_radiance();
            ensure!(
                values.len() == exact.len(),
                "capped/reference topology mismatch"
            );
            let mut peak = 0f32;
            let mut mean = 0f64;
            for (a, b) in values.iter().zip(&exact) {
                for axis in 0..3 {
                    let e = (a[axis] - b[axis]).abs() * fixture.exposure;
                    peak = peak.max(e);
                    mean += f64::from(e);
                }
            }
            let flip = if let Some(capture) = &mut capture {
                Some(capture.frame(
                    &fixture,
                    &changes,
                    &capped,
                    &uncapped,
                    &exact,
                    (frame, case.event),
                )?)
            } else {
                None
            };
            scores.push(FrameScore {
                frame,
                uncapped: demanded,
                capped: protected,
                max_error_white: peak,
                mean_error_white: (mean / (values.len().max(1) * 3) as f64) as f32,
                region_flip_excess: flip,
                measured_transport_heap_live: measured_live,
                measured_transport_heap_peak: frame_peak,
            });
            if frame % 10 == 0 || frame == case.event || frame + 1 == frames.unwrap_or(case.frames)
            {
                let last = scores.last().context("frame score")?;
                println!(
                    "G3 {} F{} uncapped_cpu_ms={:.4} capped_cpu_ms={:.4} max_white={:.6} debt_age={}",
                    case.name,
                    frame,
                    last.uncapped.event_cpu_ms,
                    last.capped.event_cpu_ms,
                    peak,
                    last.capped.oldest_debt
                );
                std::io::stdout().flush()?;
                write_json(&dir.join("frames.json"), &scores)?;
            }
        }
        replay.finish()?;
        write_json(&dir.join("frames.json"), &scores)?;
        let mut costs: Vec<_> = scores
            .iter()
            .filter(|s| s.frame >= case.event)
            .map(|s| s.uncapped.event_cpu_ms)
            .collect();
        costs.sort_by(f64::total_cmp);
        let average = costs.iter().sum::<f64>() / costs.len().max(1) as f64;
        let p99 = costs
            .get((costs.len() * 99 / 100).min(costs.len().saturating_sub(1)))
            .copied();
        let maximum = costs.last().copied();
        let max_error = scores
            .iter()
            .map(|s| s.max_error_white)
            .fold(0f32, f32::max);
        let value = serde_json::json!({"fixture":"B3 deterministic fallback; T1 catalogue read at f0e0e66fc70e287286fceed78dc76509b4de8821; T1 worker not merged",
            "case":case.name,"declared_frames":case.frames,"completed_frames":scores.len()+1,"triangles_per_body":case.triangles,
            "bodies":case.bodies,"lights":case.lights,"cold":cold,"uncapped_cpu_average_ms":average,"uncapped_cpu_p99_ms":p99,
            "uncapped_cpu_max_ms":maximum,"capped_max_error_white":max_error,
            "peak_host_transport_bytes":scores.iter().map(|s|s.capped.resident_bytes).chain([cold.resident_bytes]).max(),
            "measured_heap_peak_bytes":scores.iter().map(|s|s.measured_transport_heap_peak).chain([cold_peak]).max(),
            "measured_heap_cold_peak_bytes":cold_peak,
            "max_debt_age":scores.iter().map(|s|s.capped.oldest_debt).max(),
            "max_region_flip_excess":scores.iter().filter_map(|s|s.region_flip_excess).reduce(f64::max),
            "reference":"Independent converged FP64 patch system; current-geometry pathtrace and mover/media quality require separate image evidence",
            "760m_event_gpu_ms":null,"promotion":"not promoted"});
        write_json(&dir.join("summary.json"), &value)?;
        results.insert(case.name, value);
    }
    ensure!(!results.is_empty(), "unknown G3 case");
    write_json(&out.join("summary.json"), &results)
}

/// Explicit offline manifest edit through Ara's canonical writer. Only the three
/// owned entries are inserted; unrelated shared entries retain their exact bytes.
fn register_pipelines() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shaders/pipelines.json");
    let original = std::fs::read_to_string(&path)?;
    let parsed = ara_json::parse(&original)?;
    let mut entries = serde_json::Map::new();
    for (name, source, fragment) in [
        (
            "transport_diffuse",
            "transport-readout.slang",
            Some("transportDiffuseFragment"),
        ),
        ("transport_gather", "transport-gather.slang", None),
        (
            "transport_lighting",
            "transport-readout.slang",
            Some("transportLightingFragment"),
        ),
    ] {
        if parsed.get(name).is_some() {
            continue;
        }
        let stages = if let Some(fragment) = fragment {
            serde_json::json!({"fullscreenVertex":"vertex",fragment:"fragment"})
        } else {
            serde_json::json!({"transportGather":"compute"})
        };
        entries.insert(name.into(),serde_json::json!({"entries":stages,"source":source,"state":{
            "blend":false,"color_formats":if fragment.is_some(){vec![97]}else{vec![]},"depth_format":null,"depth_test":false,"depth_write":false,"vertex_stride":0}}));
    }
    if entries.is_empty() {
        return Ok(());
    }
    let canonical =
        ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::Value::Object(entries)));
    let snippet = canonical
        .strip_prefix("{\n")
        .and_then(|s| s.strip_suffix("}\n"))
        .context("canonical object wrapper")?;
    let index = original.rfind('}').context("pipeline manifest object")?;
    let prefix = original[..index].trim_end();
    std::fs::write(path, format!("{prefix},\n{snippet}}}\n"))?;
    Ok(())
}
