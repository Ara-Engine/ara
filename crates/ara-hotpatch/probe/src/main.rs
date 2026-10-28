//! Real renderer, fixed scene, no window. Control/captures stay under an explicit scratch folder.
mod fidelity;
use anyhow::{Context, Result, bail};
use ara_gpu::{Access, GpuDevice, Resource};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    instanced::*,
    scene::CameraSpec,
};
use glam::{Mat4, Vec3};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    // SAFETY: Native argument decoding is the first operation, before all threads.
    let arguments = unsafe { ara_hotpatch::args_os()? };
    let mut output = PathBuf::from("target/ara-hotpatch/render-probe");
    let mut start = None;
    let mut gpu_child = false;
    let mut fidelity = false;
    let mut args = arguments.iter().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--output") => {
                output = PathBuf::from(args.next().context("--output needs a directory")?)
            }
            Some("--start-file") => {
                start = Some(PathBuf::from(
                    args.next().context("--start-file needs a file")?,
                ))
            }
            Some("--gpu-child") => gpu_child = true,
            Some("--fidelity") => fidelity = true,
            _ => bail!("unknown render probe argument: {arg:?}"),
        }
    }
    // Per-process GPU attribution is Windows-only. On Mac this is a windowless
    // functional/latency proof, with one manually admitted job, not a GPU benchmark.
    if !gpu_child && cfg!(windows) {
        let driver = std::env::var_os("ARA_HOTPATCH_DRIVER")
            .context("launch this probe with ara-dev hotpatch")?;
        let mut command = std::process::Command::new(driver);
        command.args(["gpu", "--timeout", "900", "--wait-for-quiet"]);
        if std::env::var_os("ARA_HOTPATCH_LATENCY").is_some() {
            // Compilation is the measured workload; retain GPU admission/load checks.
            // This explicitly declared CPU policy cannot establish D26 frame budgets.
            command.args(["--foreign-cpu-percent", "100"]);
        }
        let status = command
            .arg("--")
            .arg(std::env::current_exe()?)
            .arg("--gpu-child")
            .args(arguments.iter().skip(1))
            .status()?;
        anyhow::ensure!(status.success(), "GPU-queued render probe failed: {status}");
        return Ok(());
    }
    std::fs::create_dir_all(&output)?;
    let mut patches = ara_hotpatch::Hotpatch::connect();
    let began = Instant::now();
    while start.as_ref().is_some_and(|p| !p.exists()) {
        if output.join("stop").exists() {
            return Ok(());
        }
        anyhow::ensure!(
            began.elapsed() < Duration::from_secs(900),
            "GPU admission start file timed out"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    if fidelity {
        return fidelity::run(output, patches, began);
    }
    let size = [320, 180];
    let mut renderer = Renderer::new_native(
        &scene(),
        Box::new(NoTextures),
        size,
        &[],
        output.join("gpu"),
    )?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 50_f32.to_radians(),
        },
        size,
        mode: ViewMode::LitWireframe,
        deterministic: true,
        exposure: 0.0015,
        settle_frames: 1,
        tick: 0,
        seed: 7,
        marks: None,
    };
    let mut frame = 0_u64;
    loop {
        let applied = patches.frame_boundary();
        let capture = renderer.capture(&request)?;
        frame += 1;
        if frame == 1 || applied {
            capture
                .write(&output.join(format!("generation-{}.png", patches.status().generation)))?;
            println!(
                "ARA_RENDER_PATCH_FRAME pid={} frame={frame} generation={} elapsed_ms={}",
                std::process::id(),
                patches.status().generation,
                began.elapsed().as_millis()
            );
        }
        std::fs::write(
            output.join("status.json"),
            serde_json::to_vec(
                &serde_json::json!({"pid":std::process::id(), "frame":frame,"rust":patches.status()}),
            )?,
        )?;
        if output.join("stop").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    renderer.gpu().assert_clean()?;
    Ok(())
}

fn scene() -> Scene {
    let mut material = Material::default();
    material.pbr.base_color = [0.15, 0.4, 0.8, 1.];
    material.pbr.textures = [u32::MAX; 4];
    Scene {
        geometry: vec![Geometry {
            key: [5; 32],
            vertices: [[-1., -0.8, 0.], [1., -0.8, 0.], [0., 0.8, 0.]]
                .map(|position| Vertex {
                    position,
                    normal: [0., 0., 1.],
                    uv: [0.; 2],
                    uv1: [0.; 2],
                    tangent: [1., 0., 0., 1.],
                    terrain_weights: 0,
                    reserved: 0,
                })
                .to_vec(),
            lods: vec![Lod {
                indices: vec![0, 1, 2],
                error: 0.,
            }],
            bounds: [Vec3::new(-1., -0.8, 0.), Vec3::new(1., 0.8, 0.)],
            deforming: false,
            revision: 0,
        }],
        instances: vec![Instance {
            entity: 1,
            name: "Rust pass proof".into(),
            geometry: 0,
            material: 0,
            world: Mat4::IDENTITY,
            previous_world: Mat4::IDENTITY,
        }],
        materials: vec![material],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::new(0.3, 0.7, 0.6).normalize(),
        sun_lux: Vec3::splat(3500.),
        environment: vec![[50., 75., 100., 1.]; 128],
        environment_size: [16, 8],
    }
}

struct NoTextures;
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
