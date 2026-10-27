use anyhow::{Result, ensure};
#[path = "benchmark/motion.rs"]
mod motion;
#[path = "benchmark/scene.rs"]
mod scene;
use ara_geo::{Geometry, Settings, cook};
use ara_gpu::{GpuDevice, cooked::write_json};
use ara_render::scene::CameraSpec;
use ara_render_geo::shadows::VirtualShadows;
use ara_render_geo::upscale::SpatialUpscaler;
use ara_render_geo::{DebugView, GeometryView, View};
use ara_texture_stream::{Catalog, Residency, Source};
use glam::Vec3;
use std::{collections::BTreeMap, path::PathBuf};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let smoke = args.iter().any(|s| s == "--smoke");
    let mesh = !args.iter().any(|s| s == "--indirect");
    let upscaled = args.iter().any(|s| s == "--upscaled");
    let hardware_only = args.iter().any(|s| s == "--hardware-only");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("captures").join(if smoke {
        "photo-smoke"
    } else if upscaled {
        "photo-upscaled1440"
    } else if hardware_only {
        "photo-native1440-hardware"
    } else if !mesh {
        "photo-native1440-indirect"
    } else {
        "photo-native1440"
    });
    std::fs::create_dir_all(&output)?;
    let (side, mut size): (u32, [u32; 2]) = if smoke {
        (4, [960, 540])
    } else {
        (40, [2560, 1440])
    };
    if upscaled {
        size = [1707, 960];
    }
    let display_size = if upscaled { [2560, 1440] } else { size };
    let (source, instances) = scene::photogrammetry(&root, side)?;
    let asset = root.join("captures/assets/coast_rocks_01/scan-ground-v1.arag");
    let geometry = if asset.exists() {
        Geometry::decode(&std::fs::read(&asset)?)?
    } else {
        let g = cook(&source, Settings::default())?;
        std::fs::write(&asset, g.encode()?)?;
        g
    };
    drop(source);
    let camera = CameraSpec::LookAt {
        position: Vec3::new(18., 8., 22.),
        target: Vec3::ZERO,
        vertical_fov: 55.0f32.to_radians(),
    };
    let mut gpu = GpuDevice::new(&[], output.join("gpu-cache"))?;
    let albedo = image::open(
        root.join("captures/assets/coast_rocks_01/textures/coast_rocks_01_diff_8k.jpg"),
    )?
    .to_rgba8();
    let catalog = Catalog::new(vec![Source::rgba8(
        [albedo.width(), albedo.height()],
        albedo.into_raw(),
        true,
        100,
    )?])?;
    let mut textures = Residency::new(
        &mut gpu,
        catalog,
        64 * 1024 * 1024,
        48,
        size,
        &root.join("../ara-texture-stream/cooked"),
    )?;
    let mut view = GeometryView::new(
        &mut gpu,
        &geometry,
        &instances,
        size,
        1_000_000,
        &root.join("cooked"),
        mesh,
    )?;
    let lights: Vec<_> = (0..3)
        .map(|level| {
            ara_vsm::directional(
                1,
                Vec3::new(0.6, -1.0, -0.3),
                Vec3::ZERO,
                level,
                side as f32 * 4.,
                1,
            )
        })
        .collect::<Result<_>>()?;
    let mut shadows = VirtualShadows::new(
        &mut gpu,
        &view,
        if smoke { 16384 } else { 4096 },
        1_000_000,
        &root.join("cooked"),
    )?;
    let mut cold_shadow = None;
    let process_vram = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()
        .filter(|r| r.status.success())
        .and_then(|r| String::from_utf8(r.stdout).ok())
        .and_then(|r| r.lines().next().and_then(|r| r.trim().parse::<u64>().ok()));
    let texture_memory = textures.stats(&gpu)?;
    let resource_bytes = texture_memory.pool_bytes
        + texture_memory.auxiliary_bytes
        + view.memory_bytes()
        + shadows.memory_bytes()
        + if upscaled {
            u64::from(display_size[0]) * u64::from(display_size[1]) * 4
        } else {
            0
        };
    let mut cold_passes = BTreeMap::new();
    let mut reconstruction = if upscaled {
        Some(SpatialUpscaler::new(
            &mut gpu,
            view.outputs().preview,
            size,
            display_size,
            &root.join("cooked"),
        )?)
    } else {
        None
    };
    let mut pass_times = BTreeMap::<String, Vec<f64>>::new();
    let mut frame_times = Vec::new();
    for frame in 0..if smoke { 24 } else { 96 } {
        textures.prepare(&gpu)?;
        let list = textures.begin_frame(&gpu)?;
        let point = gpu.submit(list, &[])?;
        let mut texture_timings = gpu.resolve(point)?;
        view.bind_textures(Some(textures.binding()))?;
        view.update(
            &mut gpu,
            &View {
                camera: &camera,
                error_pixels: 1.0,
                reset_history: frame < 2,
                debug: DebugView::Lit,
                hardware_only,
                full_resolution: false,
            },
        )?;
        let list = view.record(&gpu)?;
        let p = gpu.submit(list, &[])?;
        view.submitted(p);
        let mut timings = gpu.resolve(p)?;
        let list = textures.end_frame(&gpu)?;
        let point = gpu.submit(list, &[])?;
        texture_timings.extend(gpu.resolve(point)?);
        textures.submitted(point);
        timings.extend(texture_timings);
        shadows.update(&mut gpu, &view, &lights, Vec3::ZERO, frame == 0)?;
        let list = shadows.record(&mut gpu, &view)?;
        let p = gpu.submit(list, &[])?;
        shadows.submitted(p);
        timings.extend(gpu.resolve(p)?);
        if let Some(upscale) = &mut reconstruction {
            let list = upscale.record(&gpu)?;
            let point = gpu.submit(list, &[])?;
            timings.extend(gpu.resolve(point)?);
        }
        if frame == 0 {
            cold_shadow = Some(shadows.stats(&mut gpu, &view)?);
            cold_passes = timings
                .iter()
                .map(|p| (p.pass.to_owned(), p.milliseconds))
                .collect();
        }
        if frame >= if smoke { 16 } else { 64 } {
            frame_times.push(timings.iter().map(|t| t.milliseconds).sum::<f64>());
            for t in timings {
                pass_times.entry(t.pass).or_default().push(t.milliseconds);
            }
        }
    }
    let stats = view.stats(&mut gpu)?;
    ensure!(
        stats.overflow == 0,
        "visible-cluster queue overflow: {}",
        stats.overflow
    );
    if let Some(upscale) = &reconstruction {
        upscale.capture(&mut gpu, &output.join("lit.png"))?;
    } else {
        view.capture(&mut gpu, &output.join("lit.png"))?;
    }
    let shadow_stats = shadows.stats(&mut gpu, &view)?;
    let texture_stats = textures.stats(&gpu)?;
    ensure!(
        shadow_stats.missing_pages == 0 && shadow_stats.cluster_overflow == 0,
        "VSM capacity misses invalidate the benchmark"
    );
    shadows.capture(&mut gpu, &view, &output.join("shadows.png"))?;
    let identity = view.identities(&mut gpu)?;
    ensure!(
        identity.iter().any(|id| id[0] != 0),
        "empty geometry capture"
    );
    for (name, debug) in [
        ("clusters", DebugView::Clusters),
        ("lod", DebugView::Lod),
        ("overdraw", DebugView::Overdraw),
        ("raster", DebugView::Raster),
        ("texture-residency", DebugView::TextureMipResidency),
        ("vsm-pages", DebugView::VsmPages),
    ] {
        view.update(
            &mut gpu,
            &View {
                camera: &camera,
                error_pixels: 1.0,
                reset_history: true,
                debug,
                hardware_only,
                full_resolution: false,
            },
        )?;
        let list = view.record(&gpu)?;
        let p = gpu.submit(list, &[])?;
        view.submitted(p);
        gpu.resolve(p)?;
        if debug == DebugView::VsmPages {
            shadows.update(&mut gpu, &view, &lights, Vec3::ZERO, false)?;
            let list = shadows.record(&mut gpu, &view)?;
            let point = gpu.submit(list, &[])?;
            shadows.submitted(point);
            gpu.resolve(point)?;
        }
        if let Some(upscale) = &mut reconstruction {
            let list = upscale.record(&gpu)?;
            let point = gpu.submit(list, &[])?;
            gpu.resolve(point)?;
            upscale.capture(&mut gpu, &output.join(format!("{name}.png")))?;
        } else {
            view.capture(&mut gpu, &output.join(format!("{name}.png")))?;
        }
    }
    let passes: BTreeMap<_, _> = pass_times
        .into_iter()
        .map(|(name, mut times)| {
            times.sort_by(f64::total_cmp);
            let n = times.len();
            (
                name,
                serde_json::json!({"p50_ms":times[n/2],"p95_ms":times[n*95/100],"samples":n}),
            )
        })
        .collect();
    frame_times.sort_by(f64::total_cmp);
    write_json(
        &output.join("benchmark.json"),
        &serde_json::json!({"settings":ara_render_geo::settings::RenderSettings::default(),"device":gpu.info(),"size":size,"display_size":display_size,"requested_resource_bytes":resource_bytes,"texture_memory":texture_stats,"system_vram_mib_after_allocation":process_vram,"reconstruction":if upscaled{"linear-light bilinear"}else{"native"},"hardware_only":hardware_only,"mesh_shaders":mesh&&gpu.info().mesh_shading,"stats":stats,"geometry_warnings":geometry.warnings(),"vsm_cold":cold_shadow,"vsm_warm":shadow_stats,"cold_passes_ms":cold_passes,"cold_total_ms":cold_passes.values().sum::<f64>(),"passes":passes,"total_p50_ms":frame_times[frame_times.len()/2],"instance_count":instances.len(),"license":"Poly Haven Coast Rocks 01 CC0; ASSET-LICENSES.md","background_load":"Shared workstation, CARGO_BUILD_JOBS=2; no claim of exclusive GPU access","limitations":["Preview lighting is not fidelity integration","Frozen camera and static casters for warm numbers","Whole-level geometry fallback when a cluster page budget is insufficient","All cluster pages resident; texture pool limited to 64 MiB","679936 photographic triangles and two ground triangles per instance"]}),
    )?;
    if args.iter().any(|arg| arg == "--motion") {
        motion::measure(&mut gpu, &mut view, &mut textures, &output, hardware_only)?;
    }
    gpu.assert_clean()?;
    if let Some(upscale) = reconstruction {
        upscale.retire(&mut gpu)?;
    }
    shadows.retire(&mut gpu)?;
    view.retire(&mut gpu)?;
    textures.retire(&mut gpu)?;
    println!("{}", output.display());
    Ok(())
}
