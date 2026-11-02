//! Native-resolution deterministic readout captures and independent path tracing.
use super::{fixtures::Fixture, gltf_scene, memory, write_json};
use anyhow::Result;
use ara_render::{
    PatchTransport, Renderer, TransportOptions,
    capture::{CaptureRequest, ViewMode},
};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use std::path::{Path, PathBuf};

const SIZE: [u32; 2] = [1920, 1080];
fn request(f: &Fixture) -> CaptureRequest {
    CaptureRequest {
        camera: f.camera,
        size: SIZE,
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: f.exposure,
        settle_frames: 1,
        tick: 0,
        seed: 0,
        marks: None,
    }
}

pub struct ReplayImages {
    renderer: Renderer,
    out: PathBuf,
}
impl ReplayImages {
    pub fn new(f: &Fixture, out: &Path) -> Result<Self> {
        Ok(Self {
            renderer: Renderer::new_raster(gltf_scene::world(f), SIZE, &[], out.join("gpu"))?,
            out: out.to_owned(),
        })
    }
    pub fn frame(
        &mut self,
        f: &Fixture,
        changes: &ara_render::instanced::Changes,
        transport: &PatchTransport,
        uncapped: &PatchTransport,
        exact: &[[f32; 3]],
        (frame, event): (u64, u64),
    ) -> Result<f64> {
        if !changes.instances.is_empty()
            || !changes.materials.is_empty()
            || !changes.geometries.is_empty()
        {
            self.renderer.replace_scene(gltf_scene::world(f))?;
        } else {
            self.renderer.update_geometry(gltf_scene::world(f))?;
        }
        self.renderer
            .set_patch_transport(Some(transport), &f.scene)?;
        let candidate = self.renderer.capture(&request(f))?;
        self.renderer
            .set_patch_transport_reference(uncapped, &f.scene, exact)?;
        let reference = self.renderer.capture(&request(f))?;
        let score = ara_pathtrace::flip::flip(
            &reference.image,
            &candidate.image,
            ara_pathtrace::flip::PIXELS_PER_DEGREE,
        )?;
        // Maximum 64x64 region mean, rather than a full-frame mean hiding local debt.
        let mut worst = 0f64;
        for y in (0..SIZE[1]).step_by(64) {
            for x in (0..SIZE[0]).step_by(64) {
                let mut sum = 0f64;
                let mut count = 0;
                for yy in y..(y + 64).min(SIZE[1]) {
                    for xx in x..(x + 64).min(SIZE[0]) {
                        sum += f64::from(score.errors[(yy * SIZE[0] + xx) as usize]);
                        count += 1;
                    }
                }
                worst = worst.max(sum / f64::from(count));
            }
        }
        if (event..=event + 3).contains(&frame) || frame == 1 {
            candidate
                .image
                .save(self.out.join(format!("F{frame:03}-capped.png")))?;
            reference
                .image
                .save(self.out.join(format!("F{frame:03}-exact-patch.png")))?;
            write_json(
                &self.out.join(format!("F{frame:03}-capture.json")),
                &candidate.metadata,
            )?;
        }
        Ok(worst)
    }
}

pub fn ab(f: &Fixture, out: &Path, samples: u32) -> Result<()> {
    let mut renderer = Renderer::new_raster(gltf_scene::world(f), SIZE, &[], out.join("raster"))?;
    let mut gpu = ara_gpu::GpuDevice::new(&[], out.join("reference-gpu"))?;
    let world = gltf_scene::world(f);
    let post = PostSettings {
        bloom: 0.,
        fog_density: 0.,
        aperture: 0.,
        shutter: 0.,
        ..PostSettings::default()
    };
    let settings = Settings {
        render_size: SIZE,
        display_size: SIZE,
        tier: Tier::Rt,
        mode: Mode::Reference {
            samples: 1,
            bounces: 16,
        },
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 1.,
        post,
    };
    let mut acquire = FidelityView::new(&mut gpu, &world, settings, None)?;
    acquire.render(
        &mut gpu,
        &FrameInput {
            camera: &f.camera,
            tick: 0,
            delta_seconds: 1. / 60.,
            reset: true,
            lighting_changed: false,
            deterministic: true,
        },
    )?;
    let probes = acquire.bake_probe_surfaces(&mut gpu)?;
    acquire.destroy(&mut gpu)?;
    renderer.set_probe_surfaces(Some(&probes))?;
    let mut req = request(f);
    req.settle_frames = 8;
    let probe = renderer.capture(&req)?;
    let (_, probe_hdr) = renderer.lab_radiance()?;
    probe.image.save(out.join("probe-gi.png"))?;
    write_json(&out.join("probe-gi.json"), &probe.metadata)?;
    let probe_timing = steady(&mut renderer, &req)?;
    let mut reference = ara_pathtrace::Reference::new(&mut gpu, &world, SIZE, samples, 16, post)?;
    let mut half = None;
    let mut batches = 0;
    loop {
        let complete = reference.step(&mut gpu, &f.camera, false)?;
        batches += 8;
        if half.is_none() && batches >= samples / 2 {
            half = Some(reference.radiance(&mut gpu)?);
        }
        if complete {
            break;
        }
    }
    let reference_hdr = reference.radiance(&mut gpu)?;
    reference
        .capture(&mut gpu)?
        .save(out.join("pathtrace.png"))?;
    reference.destroy(&mut gpu)?;
    let convergence = half.as_ref().map(|half| ratio(&reference_hdr, half));
    let mut metrics = serde_json::Map::new();
    metrics.insert(
        "probe_gi".into(),
        serde_json::json!({"mean_abs_ln_region_ratio":ratio(&reference_hdr,&probe_hdr),"steady_gpu":probe_timing}),
    );
    for global in [false, true] {
        let name = if global {
            "patch-global"
        } else {
            "patch-per-patch"
        };
        println!("A/B building {name}");
        let before = memory::begin();
        let transport = PatchTransport::new(
            &f.scene,
            &f.textures,
            BTreeMap::new(),
            TransportOptions {
                global_directions: global,
                ..TransportOptions::default()
            },
            Some(&out.join("charts")),
        )?;
        let (heap_live, heap_peak) = memory::end(before);
        let publication = std::time::Instant::now();
        renderer.set_patch_transport(Some(&transport), &f.scene)?;
        let publication_ms = publication.elapsed().as_secs_f64() * 1000.;
        let capture = renderer.capture(&req)?;
        let (_, hdr) = renderer.lab_radiance()?;
        capture.image.save(out.join(format!("{name}.png")))?;
        write_json(&out.join(format!("{name}.json")), &capture.metadata)?;
        let timing = steady(&mut renderer, &req)?;
        metrics.insert(name.into(),serde_json::json!({"mean_abs_ln_region_ratio":ratio(&reference_hdr,&hdr),"report":transport.report(),"gpu_readout_bytes":renderer.patch_transport_gpu_bytes(),"measured_heap_live_bytes":heap_live,"measured_heap_peak_bytes":heap_peak,"publication_cpu_ms":publication_ms,"steady_gpu":timing}));
    }
    write_json(
        &out.join("metrics.json"),
        &serde_json::json!({"size":SIZE,"samples":samples,"reference_converged":false,"half_vs_full_mean_abs_ln_region_ratio":convergence,"reference_bounces":16,"region":"64x64 screen regions, mean Rec.709 luminance, floor 1e-6","source":"source glTF basic PBR with common lighting; extension materials and cooked Polar authoring are not reproduced","metrics":metrics}),
    )
}
use std::collections::BTreeMap;
fn steady(renderer: &mut Renderer, request: &CaptureRequest) -> Result<serde_json::Value> {
    let mut request = request.clone();
    request.settle_frames = 1;
    let mut lighting = Vec::new();
    let mut frames = Vec::new();
    for frame in 0..180 {
        let capture = renderer.capture(&request)?;
        if frame < 30 {
            continue;
        }
        let passes: Vec<_> = capture
            .metadata
            .timings
            .iter()
            .filter(|t| t.pass != "CaptureReadback")
            .collect();
        if let Some(pass) = passes.iter().find(|t| t.pass == "DeferredLighting") {
            lighting.push(pass.milliseconds);
        }
        let start = passes
            .iter()
            .map(|t| t.start_ms)
            .fold(f64::INFINITY, f64::min);
        let end = passes.iter().map(|t| t.end_ms).fold(0., f64::max);
        frames.push(end - start);
    }
    let quantiles = |mut samples: Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        serde_json::json!({"samples":samples.len(),"p50_ms":samples.get(samples.len()/2),
            "p99_ms":samples.get(samples.len()*99/100),"max_ms":samples.last()})
    };
    Ok(
        serde_json::json!({"deferred_including_direct_and_specular":quantiles(lighting),"render_span_excluding_readback":quantiles(frames)}),
    )
}
fn ratio(reference: &[[f32; 4]], candidate: &[[f32; 4]]) -> f64 {
    let mut total = 0.;
    let mut count = 0;
    for y in (0..SIZE[1]).step_by(64) {
        for x in (0..SIZE[0]).step_by(64) {
            let mut a = 0f64;
            let mut b = 0f64;
            for yy in y..(y + 64).min(SIZE[1]) {
                for xx in x..(x + 64).min(SIZE[0]) {
                    let i = (yy * SIZE[0] + xx) as usize;
                    for (c, w) in [0.2126, 0.7152, 0.0722].into_iter().enumerate() {
                        a += f64::from(reference[i][c]) * w;
                        b += f64::from(candidate[i][c]) * w;
                    }
                }
            }
            total += (a.max(1e-6) / b.max(1e-6)).ln().abs();
            count += 1;
        }
    }
    total / f64::from(count)
}
