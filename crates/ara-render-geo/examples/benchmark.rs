use anyhow::{Result, ensure};
use ara_geo::{Geometry, Settings, cook, sample};
use ara_gpu::{GpuDevice, cooked::write_json};
use ara_render::scene::CameraSpec;
use ara_render_geo::shadows::VirtualShadows;
use ara_render_geo::upscale::SpatialUpscaler;
use ara_render_geo::{DebugView, GeometryView, Instance, View};
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
        "smoke"
    } else if upscaled {
        "upscaled1440"
    } else if hardware_only {
        "native1440-hardware"
    } else if !mesh {
        "native1440-indirect"
    } else {
        "native1440"
    });
    std::fs::create_dir_all(&output)?;
    let (rings, segments, side, mut size): (u32, u32, u32, [u32; 2]) = if smoke {
        (48, 96, 10, [960, 540])
    } else {
        (710, 710, 32, [2560, 1440])
    };
    if upscaled {
        size = [1707, 960];
    }
    let display_size = if upscaled { [2560, 1440] } else { size };
    let asset = output.join(format!("rock-{rings}-{segments}.arag"));
    let geometry = if asset.exists() {
        Geometry::decode(&std::fs::read(&asset)?)?
    } else {
        let g = cook(&sample::rock(rings, segments), Settings::default())?;
        std::fs::write(&asset, g.encode()?)?;
        g
    };
    let mut instances = Vec::new();
    for z in 0..side {
        for x in 0..side {
            let h = x
                .wrapping_mul(747796405u32)
                .wrapping_add(z.wrapping_mul(2891336453));
            let r = (h >> 8) & 255;
            instances.push(Instance {
                position_scale: [
                    (x as f32 - side as f32 * 0.5) * 3.0,
                    0.0,
                    (z as f32 - side as f32 * 0.5) * 3.0,
                    0.7 + r as f32 / 255.0 * 0.5,
                ],
                color_roughness: [0.3 + r as f32 / 255.0 * 0.15, 0.25, 0.19, 0.75],
                identity: [(z * side + x + 1), 0, 0, 0],
            });
        }
    }
    let camera = CameraSpec::LookAt {
        position: Vec3::new(side as f32 * 1.2, side as f32 * 0.8, side as f32 * 1.5),
        target: Vec3::ZERO,
        vertical_fov: 55.0f32.to_radians(),
    };
    let mut gpu = GpuDevice::new(&[], output.join("gpu-cache"))?;
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
                side as f32,
                1,
            )
        })
        .collect::<Result<_>>()?;
    let mut shadows = VirtualShadows::new(
        &mut gpu,
        &view,
        if smoke { 8192 } else { 16384 },
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
    let resource_bytes = view.memory_bytes()
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
    for frame in 0..if smoke { 12 } else { 40 } {
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
        if frame >= 8 {
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
        &serde_json::json!({"device":gpu.info(),"size":size,"display_size":display_size,"requested_resource_bytes":resource_bytes,"system_vram_mib_after_allocation":process_vram,"reconstruction":if upscaled{"linear-light bilinear"}else{"native"},"hardware_only":hardware_only,"mesh_shaders":mesh&&gpu.info().mesh_shading,"stats":stats,"geometry_warnings":geometry.warnings(),"vsm_cold":cold_shadow,"vsm_warm":shadow_stats,"cold_passes_ms":cold_passes,"cold_total_ms":cold_passes.values().sum::<f64>(),"passes":passes,"total_p50_ms":frame_times[frame_times.len()/2],"instances":instances.iter().enumerate().map(|(index,i)|serde_json::json!({"render_index":index+1,"entity_low":i.identity[0],"entity_high":i.identity[1]})).collect::<Vec<_>>(),"license":"Original procedural geometry; no external media","background_load":"Shared workstation, CARGO_BUILD_JOBS=2; no claim of exclusive GPU access","limitations":["Preview lighting is not fidelity integration","Frozen camera and static casters for warm numbers","Whole-level residency fallback when a page budget is insufficient","All cluster pages resident in this benchmark"]}),
    )?;
    gpu.assert_clean()?;
    if let Some(upscale) = reconstruction {
        upscale.retire(&mut gpu)?;
    }
    shadows.retire(&mut gpu)?;
    view.retire(&mut gpu)?;
    println!("{}", output.display());
    Ok(())
}
