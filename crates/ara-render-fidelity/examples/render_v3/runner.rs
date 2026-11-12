use super::{contract::*, coverage, fixture, replay};
use anyhow::{Result, ensure};
use ara_frame::capture::{CaptureRequest, ViewMode};
use ara_gpu::{Device, GpuDevice};
use ara_render::Renderer;
use ara_render_fidelity::{
    FidelityView, Mode, PostSettings, Settings, Tier, Upscaler, presets::QualityPreset,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

pub fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() >= 4,
        "render_v3 CASE.json uncapped|capped OUTPUT WIDTH HEIGHT [SAMPLES] [all-references]"
    );
    let case: Case = serde_json::from_slice(&fs::read(&args[0])?)?;
    let suite: Suite = serde_json::from_str(include_str!(
        "../../../ara-dev/src/lab/stress/cases.v1.json"
    ))?;
    ensure!(
        suite.version == VERSION && suite.cases.iter().any(|c| c.recipe == case.recipe),
        "unknown stress recipe/version"
    );
    let mode = match args[1].as_str() {
        "uncapped" => RunKind::Uncapped,
        "capped" => RunKind::Capped,
        _ => anyhow::bail!("unknown run kind"),
    };
    let output = PathBuf::from(&args[2]);
    let width = args[3].parse()?;
    let height = args.get(4).map_or(Ok(1080), |v| v.parse())?;
    let samples = args.get(5).map_or(Ok(256), |v| v.parse())?;
    ensure!(
        (16..=2560).contains(&width) && (16..=1440).contains(&height) && samples >= 1,
        "invalid extent/sample count"
    );
    fs::create_dir_all(&output)?;
    let result = run(
        &case,
        mode,
        &output,
        [width, height],
        samples,
        args.get(6).is_some_and(|v| v == "all-references"),
    )?;
    ara_gpu::cooked::write_json(&output.join("run.json"), &result)?;
    eprintln!(
        "host update/render wall ms: {:?}",
        distribution(result.frames.iter().filter_map(|f| f.host_update_render_ms))
    );
    Ok(())
}

fn run(
    case: &Case,
    mode: RunKind,
    output: &Path,
    size: [u32; 2],
    samples: u32,
    all_refs: bool,
) -> Result<RunResult> {
    let script = case.script(60);
    let replay_path = output
        .parent()
        .ok_or_else(|| anyhow::anyhow!("case output parent"))?
        .join("changes.bin");
    let replay_manifest = replay_path.with_extension("json");
    let identity = serde_json::json!({"case":case,"revision":revision(),"recipe_version":VERSION});
    if !replay_path.is_file()
        || fs::read(&replay_manifest)
            .ok()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
            .as_ref()
            != Some(&identity)
    {
        replay::record(
            case,
            std::io::BufWriter::new(fs::File::create(&replay_path)?),
        )?;
        ara_gpu::cooked::write_json(&replay_manifest, &identity)?;
    }
    let mut replay = replay::Replay::new(std::io::BufReader::new(fs::File::open(&replay_path)?))?;
    ara_gpu::cooked::write_json(
        &output.join("sequence.json"),
        &serde_json::json!({"version":VERSION,"case":case,"fps":60,"frames":script}),
    )?;
    let mut f = fixture::create(case)?;
    if let Some(first) = script.first() {
        fixture::apply(&mut f, case, first);
    }
    let mut renderer = Renderer::new_native(
        &f.scene,
        Box::new(super::polar::residency(&f)?),
        size,
        &[],
        output.join("gpu-cache"),
    )?;
    renderer.set_quality(control_quality(mode))?;
    let mut look = *renderer.look();
    look.auto_exposure = None;
    look.bloom = 0.;
    renderer.set_look(look)?;
    renderer.set_jitter_override(Some([0.; 2]));
    let exposure = ara_render_fidelity::settings::exposure_from_ev100(f.ev100, 0.);
    renderer.set_tone_curve(ara_render::renderer::post::ToneCurve::Neutral);
    // Acquire the current renderer's startup surface cache before measuring.
    // This is offline acquisition, not hardware RT in the scored frame loop.
    let surfaces = startup_surfaces(&f, &output.join("startup-gi-cache"))?;
    renderer.set_probe_surfaces(Some(&surfaces))?;
    renderer.set_unseen(true);
    let mut result=RunResult {version:VERSION,case:case.clone(),run:mode,renderer:"today-native-raster-high-startup-probes-no-frame-hardware-rt".into(),
        device:renderer.gpu().info().name.clone(),revision:revision(),size,fps:60,uncapped_supported:false,caps_applied:mode==RunKind::Capped,
        timing_valid:std::env::var("ARA_RENDER_V3_TIMING_VALID").is_ok_and(|v|v=="1"),reference_samples:samples,reference_bounces:8,reference_converged:false,reference_white:1./f64::from(exposure),
        notes:vec!["Control uncapped mode raises VSM redraws to the API's 256-page maximum and local redraws to all allocated faces. VSM and other hidden limits remain; this is not true uncapped demand. Stage demand, debt and deadlines remain n/a.".into(),
            "Startup probe surfaces are acquired offline with the existing fidelity baker, then relit by today's raster renderer. The runtime host's asynchronous CPU surface reacquisition is not connected: geometry/material changes retain their startup surfaces. This is a renderer-core control, not a complete Play-host/G3 control; no discovery/repair timing or correctness is claimed.".into(),
            "The path-traced reference left with ara-pathtrace. Exact max-error stays n/a.".into(),
            "gpu_frame_ms spans the update and render submissions, including GPU gaps. Pass sums and CPU update/render wall time are separate; CPU critical-path attribution is unavailable.".into(),
            "Fixture motions are deterministic scripted stress, not a physics simulation; foliage uses moving leaf cards. Polar loads the real pinned model and cooked textures in a controlled finite-range light rig.".into()],frames:Vec::new()};
    for _ in 0..16 {
        renderer.render(&f.camera, exposure)?;
    }
    renderer.set_unseen(false);
    for frame in &script {
        renderer.gpu_mut().reset_timing_window();
        let started = Instant::now();
        let layout = (f.scene.geometry.len(), f.scene.instances.len());
        let events = replay.apply(frame.frame, &mut f.scene)?;
        fixture::camera(&mut f, case, frame);
        if frame.cold_residency || layout != (f.scene.geometry.len(), f.scene.instances.len()) {
            renderer.replace_native_scene(&f.scene, Box::new(super::polar::residency(&f)?))?;
        } else {
            renderer.update_native(&f.scene)?;
        }
        if frame.camera_cut {
            renderer.reset_temporal_history();
        }
        renderer.render(&f.camera, exposure)?;
        let wall = started.elapsed().as_secs_f64() * 1000.;
        let mut stages: BTreeMap<String, StageCounters> = BTreeMap::new();
        for timing in renderer.timings() {
            let stage = stages.entry(timing.pass.to_string()).or_default();
            *stage.gpu_ms.get_or_insert(0.) += timing.milliseconds;
        }
        for name in [
            "local_lights",
            "local_pages",
            "sun_pages",
            "dynamic_pages",
            "link_repair",
            "event_solve",
            "event_total",
            "bounce_readout",
            "patch_sources",
            "transport_memory",
        ] {
            stages.entry(name.into()).or_default();
        }
        if let Some(ms) = renderer.native_update_gpu_ms() {
            stages.insert(
                "native_upload".into(),
                StageCounters {
                    gpu_ms: Some(ms),
                    ..Default::default()
                },
            );
        }
        let mut row = FrameResult {
            frame: frame.frame,
            stages,
            gpu_frame_ms: renderer
                .gpu()
                .timing_window()
                .map(ara_gpu::vulkan::TimestampSpan::milliseconds),
            cpu_critical_path_ms: None,
            submission_stall_ms: None,
            host_update_render_ms: Some(wall),
            lpp: None,
            receiver_coverage: None,
            quality: Quality::default(),
            capture: None,
            reference: None,
            evidence: BTreeMap::new(),
        };
        row.evidence
            .insert("fixture_extent_m".into(), f64::from(f.extent));
        row.evidence
            .insert("authored_ev100".into(), f64::from(f.ev100));
        if case.recipe == "polar" {
            row.evidence.insert("real_polar_asset".into(), 1.);
        }
        if let Some(gaps) = renderer.gpu().timing_window_gaps() {
            row.evidence
                .insert("gpu_inter_submission_gaps_ms".into(), gaps);
        }
        row.evidence.insert(
            "change_feed_instance_events".into(),
            events.instances.len() as f64,
        );
        row.evidence.insert(
            "change_feed_light_events".into(),
            events.lights.len() as f64,
        );
        if let Some(work) = renderer.native_frame_work() {
            row.evidence
                .insert("native_instance_rows".into(), work.instance_rows as f64);
            row.evidence
                .insert("native_cull_tests".into(), work.cull_tests as f64);
        }
        if mode == RunKind::Capped && (all_refs || case.reference_frames.contains(&frame.frame)) {
            let request = CaptureRequest {
                camera: f.camera,
                size,
                mode: ViewMode::Lit,
                deterministic: true,
                exposure,
                settle_frames: 0,
                tick: frame.frame as u64,
                seed: 0,
                marks: None,
            };
            let capture = renderer.capture_completed(&request)?;
            if case.target_lpp.is_some() {
                let coverage = coverage::lpp(&f, &capture, size)?;
                row.lpp = (coverage.unresolved_pixels == 0).then_some(coverage.lpp_lower);
                row.receiver_coverage = Some(coverage.receiver_fraction);
                row.evidence.insert("lpp_lower".into(), coverage.lpp_lower);
                row.evidence.insert("lpp_upper".into(), coverage.lpp_upper);
                row.evidence.insert(
                    "lpp_unresolved_edge_pixels".into(),
                    coverage.unresolved_pixels as f64,
                );
            }
            let capture_path = output.join(format!("frame-{:04}.png", frame.frame));
            save_png(&capture.image, &capture_path)?;
            row.evidence
                .insert("path_traced_reference_removed".into(), 1.);
            row.capture = Some(capture_path.to_string_lossy().into());
        }
        result.frames.push(row);
        if frame.frame % 30 == 0 {
            eprintln!("{} {mode:?} frame {}/{}", case.id, frame.frame, case.frames);
        }
    }
    let assets=f.scene.geometry.iter().map(|g|serde_json::json!({"key":g.key,"vertices":g.vertices.iter().map(|v|serde_json::json!({"position":v.position,"normal":v.normal,"uv":v.uv,"uv1":v.uv1,"tangent":v.tangent,"color":v.color,"terrain_weights":v.terrain_weights})).collect::<Vec<_>>(),"indices":g.lods[0].indices})).collect::<Vec<_>>();
    ara_gpu::cooked::write_json(&output.join("geometry.json"), &assets)?;
    ara_gpu::cooked::write_json(&output.join("textures.json"), &f.textures.iter().map(|(key,source)|serde_json::json!({"blake3":key,"size":source.size(),"mips":source.mip_count()})).collect::<Vec<_>>())?;
    renderer.gpu_mut().collect()?;
    renderer.gpu().assert_clean()?;
    Ok(result)
}

/// Maximize the existing API's work allowances without changing allocation or quality.
fn control_quality(mode: RunKind) -> ara_frame::look::Quality {
    let mut quality = QualityPreset::High.raster_quality();
    quality.ray_queries = None;
    if mode == RunKind::Uncapped {
        if let Some(ref mut v) = quality.virtual_shadows {
            // Quality::validate limits this dispatch to 256 pages even if the
            // atlas owns more. Keep uncapped_supported=false for this control.
            v.budget = v.pages.min(256);
        }
        if let Some(ref mut l) = quality.local_shadows {
            l.redraws = l.lights * 6;
        }
    }
    quality
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_control_modes_obey_the_renderers_dispatch_limits() {
        for mode in [RunKind::Uncapped, RunKind::Capped] {
            let quality = control_quality(mode);
            quality.validate().unwrap();
            assert!(quality.ray_queries.is_none());
            assert_eq!(
                quality.virtual_shadows.unwrap().pages,
                QualityPreset::High
                    .raster_quality()
                    .virtual_shadows
                    .unwrap()
                    .pages
            );
        }
    }
}

/// The same optional startup acquisition used by the runtime, outside replay timing.
fn startup_surfaces(
    f: &fixture::Fixture,
    cache: &Path,
) -> Result<ara_frame::look::gi::ProbeSurfaces> {
    let mut gpu = GpuDevice::new(&[], cache)?;
    let mut view = FidelityView::new_native(
        &mut gpu,
        &f.scene,
        super::polar::residency(f)?,
        Settings {
            render_size: [64, 36],
            display_size: [64, 36],
            tier: Tier::Rt,
            mode: Mode::Hybrid,
            upscaler: Upscaler::Taau,
            probe_ray_budget: 64,
            reflection_cutoff: 1.,
            post: PostSettings::default(),
        },
        None,
    )?;
    for _ in 0..256 {
        if view.refine_native_textures(&mut gpu)?.pending_textures == 0 {
            break;
        }
    }
    view.render(
        &mut gpu,
        &ara_render_fidelity::FrameInput {
            camera: &f.camera,
            tick: 0,
            delta_seconds: 1. / 60.,
            reset: true,
            lighting_changed: false,
            deterministic: true,
        },
    )?;
    let surfaces = view.bake_probe_surfaces(&mut gpu);
    view.destroy(&mut gpu)?;
    surfaces
}

fn save_png(image: &image::RgbaImage, path: &Path) -> Result<()> {
    if image.width() > 1920 || image.height() > 1080 {
        image::DynamicImage::ImageRgba8(image.clone())
            .thumbnail(1920, 1080)
            .save(path)?;
    } else {
        image.save(path)?;
    }
    Ok(())
}
fn revision() -> String {
    use sha2::{Digest, Sha256};
    std::env::current_exe()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .unwrap_or_else(|| "unknown".into())
}
