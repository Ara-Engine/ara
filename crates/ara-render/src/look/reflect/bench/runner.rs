use super::{contract::*, coverage, fixture, metrics, replay};
use anyhow::{Result, ensure};
use ara_gpu::{Device, GpuDevice};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
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

struct Previous {
    frame: u32,
    test: Vec<[f32; 4]>,
    reference: Vec<[f32; 4]>,
}

pub fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() >= 4,
        "render_v3 CASE.json uncapped|capped OUTPUT WIDTH HEIGHT [SAMPLES] [all-references]"
    );
    let case: Case = serde_json::from_slice(&fs::read(&args[0])?)?;
    let suite: Suite = serde_json::from_str(include_str!(
        "../../../../../ara-dev/src/lab/stress/cases.v1.json"
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
    if mode == RunKind::Uncapped {
        // Let the 1 Hz guard observe the worker's final CPU counters before exit.
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
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
    let mut script = case.script(60);
    if let Ok(count) = std::env::var("ARA_B4_FRAME_LIMIT") {
        script.truncate(count.parse()?);
    }
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
    let showcase = std::env::var_os("ARA_B4_SHOWCASE").is_some();
    let reference_label = if showcase {
        format!(
            "showcase-{}",
            std::env::var("ARA_B4_SHOWCASE_SHOT").unwrap_or_else(|_| "Shot_stage".into())
        )
    } else {
        case.id.clone()
    };
    let mut f = if showcase {
        super::showcase::load()?
    } else {
        fixture::create(case)?
    };
    if let Some(first) = script.first()
        && !showcase
    {
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
    let setting =
        |name: &str, fallback: &str| std::env::var(name).unwrap_or_else(|_| fallback.into());
    let enabled = setting("ARA_B4_CONTROL", "0") != "1";
    let experiment = ara_render::SoftwareReflections {
        mirror_cut: setting("ARA_B4_MIRROR_CUT", "0.1").parse()?,
        rough_cut: setting("ARA_B4_ROUGH_CUT", "0.5").parse()?,
        rough_rays: setting("ARA_B4_RAYS", "4").parse()?,
        distance: 1000.,
        interfaces: setting("ARA_B4_INTERFACES", "8").parse()?,
    };
    let cold_started = Instant::now();
    if enabled {
        renderer.set_software_reflections(Some(experiment), &f.scene)?;
    }
    let cold_host_ms = cold_started.elapsed().as_secs_f64() * 1000.;
    ara_gpu::cooked::write_json(
        &output.join("experiment.json"),
        &serde_json::json!({"enabled":enabled,"mirror_cut":experiment.mirror_cut,"rough_cut":experiment.rough_cut,"rough_rays":experiment.rough_rays,"interfaces":experiment.interfaces,"cold_host_ms":cold_host_ms,"prefiltered_source":"today probe volume and captures","native_extent":size}),
    )?;
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
    let mut result=RunResult {version:VERSION,case:case.clone(),run:mode,renderer:if enabled {"B4-software-secondary-native-fixed-GGX-ordered-glass".into()} else {"B4-same-binary-SSR-capture-control".into()},
        device:renderer.gpu().info().name.clone(),revision:revision(),size,fps:60,uncapped_supported:false,caps_applied:mode==RunKind::Capped,
        timing_valid:std::env::var("ARA_RENDER_V3_TIMING_VALID").is_ok_and(|v|v=="1"),reference_samples:samples,reference_bounces:8,reference_converged:false,reference_white:1./f64::from(exposure),
        notes:vec!["Control uncapped mode raises VSM redraws to the API's 256-page maximum and local redraws to all allocated faces. VSM and other hidden limits remain; this is not true uncapped demand. Stage demand, debt and deadlines remain n/a.".into(),
            "Startup probe surfaces are acquired offline with the existing fidelity baker, then relit by today's raster renderer. The runtime host's asynchronous CPU surface reacquisition is not connected: geometry/material changes retain their startup surfaces. This is a renderer-core control, not a complete Play-host/G3 control; no discovery/repair timing or correctness is claimed.".into(),
            "Finite-budget undenoised ara-pathtrace references are not certified exact solutions. Exact max-error gate stays n/a until convergence evidence is supplied.".into(),
            "gpu_frame_ms spans the update and render submissions, including GPU gaps. Pass sums and CPU update/render wall time are separate; CPU critical-path attribution is unavailable.".into(),
            "Fixture motions are deterministic scripted stress, not a physics simulation; foliage uses moving leaf cards. Polar loads the real pinned model and cooked textures in a controlled finite-range light rig.".into()],frames:Vec::new()};
    for _ in 0..16 {
        renderer.render(&f.camera, exposure)?;
    }
    renderer.set_unseen(false);
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
        let empty = ara_render::instanced::transport::TransportChanges::default();
        let events = if showcase {
            &empty
        } else {
            replay.apply(frame.frame, &mut f.scene)?
        };
        if !showcase {
            fixture::camera(&mut f, case, frame);
        }
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
        let measured_span = renderer
            .gpu()
            .timing_window()
            .map(ara_gpu::vulkan::TimestampSpan::milliseconds);
        let reflection = renderer.software_reflection_counters()?;
        if let Some(c) = reflection {
            for (name, pass, demand) in [
                ("reflection_mirror", "Reflect.Mirror", c.mirror_pixels),
                ("reflection_rough", "Reflect.Rough", c.rough_pixels),
                (
                    "reflection_prefiltered",
                    "Reflect.Prefiltered",
                    c.prefiltered_pixels,
                ),
                ("ordered_glass", "ForwardTransparency", c.glass_interfaces),
            ] {
                let ms = stages.get(pass).and_then(|s| s.gpu_ms);
                stages.insert(
                    name.into(),
                    StageCounters {
                        gpu_ms: ms,
                        demand: Some(u64::from(demand)),
                        work_unit: Some(
                            if name == "ordered_glass" {
                                "interfaces"
                            } else {
                                "native pixels"
                            }
                            .into(),
                        ),
                        ..Default::default()
                    },
                );
            }
        }
        let mut row = FrameResult {
            frame: frame.frame,
            stages,
            gpu_frame_ms: measured_span,
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
        if let Some(c) = reflection {
            for (name, value) in [
                ("reflection_mirror_pixels", c.mirror_pixels),
                ("reflection_rough_pixels", c.rough_pixels),
                ("reflection_prefiltered_pixels", c.prefiltered_pixels),
                ("reflection_rays", c.rays),
                ("secondary_visibility_rays", c.visibility_rays),
                ("glass_interfaces", c.glass_interfaces),
                ("secondary_exhausted", c.exhausted),
                ("secondary_invalid_media", c.invalid_media),
            ] {
                row.evidence.insert(name.into(), f64::from(value));
            }
        }
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
            let reference_root = std::env::var_os("ARA_LAB_REFERENCES")
                .map(PathBuf::from)
                .unwrap_or_else(|| "D:/AraBench/report/images".into());
            let archived = reference_root
                .join("render-v3")
                .join(&result.revision)
                .join(format!(
                    "{reference_label}-{}x{}-{samples}spp",
                    size[0], size[1]
                ));
            fs::create_dir_all(&archived)?;
            let name = format!("reference-{:04}.png", frame.frame);
            fs::copy(&ref_path, archived.join(name))?;
            ara_gpu::cooked::write_json(
                &archived.join("provenance.json"),
                &serde_json::json!({"case":case,"worker_sha256":result.revision,"size":size,"samples":samples,"bounces":8,"ev100":f.ev100,"finite_range_window":true,"convergence_certified":false,"textures_blake3":f.textures.iter().map(|(key,_)|key).collect::<Vec<_>>()}),
            )?;
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
            if case.recipe == "roughness" && !showcase {
                let mut bands = [(0.0f64, 0u64); 16];
                for (index, &error) in flip.errors.iter().enumerate() {
                    if let Some(entity) =
                        capture.entity_at(index as u32 % size[0], index as u32 / size[0])
                        && (10..26).contains(&entity)
                    {
                        let band = &mut bands[(entity - 10) as usize];
                        band.0 += f64::from(error);
                        band.1 += 1;
                    }
                }
                for (i, (sum, count)) in bands.into_iter().enumerate() {
                    if count > 0 {
                        row.evidence
                            .insert(format!("roughness_strip_{i:02}_flip"), sum / count as f64);
                    }
                }
            }
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
    let assets=f.scene.geometry.iter().map(|g|serde_json::json!({"key":g.key,"vertices":g.vertices.iter().map(|v|serde_json::json!({"position":v.position,"normal":v.normal,"uv":v.uv,"uv1":v.uv1,"tangent":v.tangent,"color":v.color,"terrain_weights":v.terrain_weights})).collect::<Vec<_>>(),"indices":g.lods[0].indices})).collect::<Vec<_>>();
    ara_gpu::cooked::write_json(&output.join("geometry.json"), &assets)?;
    ara_gpu::cooked::write_json(&output.join("textures.json"), &f.textures.iter().map(|(key,source)|serde_json::json!({"blake3":key,"size":source.size(),"mips":source.mip_count()})).collect::<Vec<_>>())?;
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
    let cache = super::reference_cache::Cache::new(f, size, samples)?;
    if let Some(pair) = cache.read()? {
        return Ok(pair);
    }
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
            ev100: Some(f.ev100),
            tone_mapper: ToneMapper::PbrNeutral,
            bloom: 0.,
            fog_density: 0.,
            aperture: 0.,
            shutter: 0.,
            ..Default::default()
        },
    };
    let residency = super::polar::residency(f)?;
    let view = FidelityView::new_native(gpu, &f.scene, residency, settings, None)?;
    let mut reference = ara_pathtrace::Reference::from_view(view)?;
    reference.converge(gpu, &f.camera)?;
    let image = reference.capture(gpu)?;
    let data = reference.radiance(gpu)?;
    reference.destroy(gpu)?;
    cache.write(&image, &data)?;
    Ok((image, data))
}

/// Maximize the existing API's work allowances without changing allocation or quality.
fn control_quality(mode: RunKind) -> ara_render::look::Quality {
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
) -> Result<ara_render::look::gi::ProbeSurfaces> {
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
