use super::{contract::*, coverage, fixture, metrics, replay};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Device, GpuDevice, Resource};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    instanced::{TextureBinding, TextureStats, Textures},
};
use ara_render_fidelity::{
    FidelityView, Mode, PostSettings, Settings, Tier, ToneMapper, Upscaler, presets::QualityPreset,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

struct NoTextures;
struct Previous {
    frame: u32,
    test: Vec<[f32; 4]>,
    reference: Vec<[f32; 4]>,
}
impl Textures for NoTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

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
    let mut f = fixture::create(case);
    if let Some(first) = script.first() {
        fixture::apply(&mut f, case, first);
    }
    let assets=f.scene.geometry.iter().map(|g|serde_json::json!({"key":g.key,"vertices":g.vertices.iter().map(|v|serde_json::json!({"position":v.position,"normal":v.normal,"uv":v.uv,"tangent":v.tangent})).collect::<Vec<_>>(),"indices":g.lods[0].indices})).collect::<Vec<_>>();
    ara_gpu::cooked::write_json(&output.join("geometry.json"), &assets)?;
    let mut renderer = Renderer::new_native(
        &f.scene,
        Box::new(NoTextures),
        size,
        &[],
        output.join("gpu-cache"),
    )?;
    let mut quality = QualityPreset::High.raster_quality();
    quality.ray_queries = None;
    // Remove existing page work quotas where the control exposes them. Other
    // admission/transport limits remain, so this is explicitly not a true replay.
    if mode == RunKind::Uncapped {
        if let Some(ref mut v) = quality.virtual_shadows {
            v.budget = v.pages;
        }
        if let Some(ref mut l) = quality.local_shadows {
            l.redraws = l.lights * 6;
        }
    }
    renderer.set_quality(quality)?;
    let mut look = *renderer.look();
    look.auto_exposure = None;
    look.bloom = 0.;
    renderer.set_look(look)?;
    renderer.set_jitter_override(Some([0.; 2]));
    let exposure = ara_render_fidelity::settings::exposure_from_ev100(5., 0.);
    renderer.set_tone_curve(ara_render::renderer::post::ToneCurve::Neutral);
    let mut result=RunResult {version:VERSION,case:case.clone(),run:mode,renderer:"today-native-raster-high-no-hardware-rt".into(),
        device:renderer.gpu().info().name.clone(),revision:revision(),size,fps:60,uncapped_supported:false,caps_applied:mode==RunKind::Capped,
        timing_valid:std::env::var("ARA_RENDER_V3_TIMING_VALID").is_ok_and(|v|v=="1"),reference_samples:samples,reference_bounces:8,reference_converged:false,reference_white:1./f64::from(exposure),
        notes:vec!["Control uncapped mode removes exposed page redraw quotas only; stage demand, hidden caps, debt and deadlines remain n/a.".into(),
            "Finite-budget undenoised ara-pathtrace references are not certified exact solutions. Exact max-error gate stays n/a until convergence evidence is supplied.".into(),
            "Native render passes are reported individually. Their sum is not a complete-frame GPU span; CPU update/render wall time is not CPU critical-path attribution.".into(),
            "Fixture motions are deterministic scripted stress, not a physics simulation; foliage uses moving leaf cards. Polar fixture is a procedural floor, not the external Polar asset.".into()],frames:Vec::new()};
    for _ in 0..16 {
        renderer.render(&f.camera, exposure)?;
    }
    let mut previous: Option<Previous> = None;
    let mut reference_gpu = if mode == RunKind::Capped {
        Some(GpuDevice::new(&[], output.join("reference-cache"))?)
    } else {
        None
    };
    for frame in &script {
        renderer.gpu_mut().reset_timing_window();
        let started = Instant::now();
        let layout = (f.scene.geometry.len(), f.scene.instances.len());
        let events = replay.apply(frame.frame, &mut f.scene)?;
        fixture::camera(&mut f, case, frame);
        if frame.cold_residency || layout != (f.scene.geometry.len(), f.scene.instances.len()) {
            renderer.replace_native_scene(&f.scene, Box::new(NoTextures))?;
        } else {
            renderer.update_native(&f.scene)?;
        }
        if frame.camera_cut {
            renderer.reset_temporal_history();
        }
        renderer.render(&f.camera, exposure)?;
        let wall = started.elapsed().as_secs_f64() * 1000.;
        let mut stages: BTreeMap<String, StageCounters> = renderer
            .timings()
            .iter()
            .map(|t| {
                (
                    t.pass.to_string(),
                    StageCounters {
                        gpu_ms: Some(t.milliseconds),
                        ..Default::default()
                    },
                )
            })
            .collect();
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
                let (lpp, coverage) = coverage::lpp(&f, &capture, size)?;
                row.lpp = Some(lpp);
                row.receiver_coverage = Some(coverage);
            }
            let (_, radiance) = renderer.lab_radiance()?;
            let capture_path = output.join(format!("frame-{:04}.png", frame.frame));
            save_png(&capture.image, &capture_path)?;
            let (image, reference) = reference(
                &f,
                reference_gpu
                    .as_mut()
                    .expect("capped runs own a reference device"),
                size,
                samples,
            )?;
            let ref_path = output.join(format!("reference-{:04}.png", frame.frame));
            save_png(&image, &ref_path)?;
            let temporal = previous
                .as_ref()
                .filter(|p| p.frame + 1 == frame.frame)
                .map(|p| (p.test.as_slice(), p.reference.as_slice()));
            row.quality = metrics::compare(
                size,
                &radiance,
                &reference,
                result.reference_white,
                temporal,
            )?;
            let flip = ara_pathtrace::flip::flip(
                &image,
                &capture.image,
                ara_pathtrace::flip::PIXELS_PER_DEGREE,
            )?;
            row.quality.region_flip = metrics::regions(&flip.errors, size);
            let err = radiance
                .iter()
                .zip(&reference)
                .flat_map(|(a, b)| {
                    (0..3).map(move |i| f64::from((a[i] - b[i]).abs()) / result.reference_white)
                })
                .fold(0., f64::max);
            row.evidence
                .insert("finite_reference_max_error_white".into(), err);
            row.capture = Some(capture_path.to_string_lossy().into());
            row.reference = Some(ref_path.to_string_lossy().into());
            previous = Some(Previous {
                frame: frame.frame,
                test: radiance,
                reference,
            });
        }
        result.frames.push(row);
        if frame.frame % 30 == 0 {
            eprintln!("{} {mode:?} frame {}/{}", case.id, frame.frame, case.frames);
        }
    }
    renderer.gpu_mut().collect()?;
    renderer.gpu().assert_clean()?;
    Ok(result)
}

fn reference(
    f: &fixture::Fixture,
    gpu: &mut GpuDevice,
    size: [u32; 2],
    samples: u32,
) -> Result<(image::RgbaImage, Vec<[f32; 4]>)> {
    let settings = Settings {
        render_size: size,
        display_size: size,
        tier: Tier::Rt,
        mode: Mode::Reference {
            samples,
            bounces: 8,
        },
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 1.,
        post: PostSettings {
            ev100: Some(5.),
            tone_mapper: ToneMapper::PbrNeutral,
            bloom: 0.,
            fog_density: 0.,
            aperture: 0.,
            shutter: 0.,
            ..Default::default()
        },
    };
    let residency = ara_texture_stream::NativeResidency::new(ara_texture_stream::NativeConfig {
        resident_bytes: 1 << 20,
        upload_bytes: 1 << 20,
    })?;
    let view = FidelityView::new_native(gpu, &f.scene, residency, settings, None)?;
    let mut reference = ara_pathtrace::Reference::from_view(view)?;
    reference.converge(gpu, &f.camera)?;
    let image = reference.capture(gpu)?;
    let data = reference.radiance(gpu)?;
    reference.destroy(gpu)?;
    Ok((image, data))
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
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|v| v.status.success())
        .and_then(|v| String::from_utf8(v.stdout).ok())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
