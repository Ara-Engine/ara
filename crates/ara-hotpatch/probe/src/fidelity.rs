//! Real fidelity recording, including its synchronous graph-building closures.
use anyhow::{Result, ensure};
use ara_gpu::{Device, GpuDevice};
use ara_hotpatch::Hotpatch;
use ara_render::scene::{CameraSpec, Material, Mesh, Scene, Texture, Vertex};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use glam::Vec3;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// Keep one view/device alive across patches, with fixed transport inputs.
pub(super) fn run(output: PathBuf, mut patches: Hotpatch, began: Instant) -> Result<()> {
    let size = [320, 180];
    let mut gpu = GpuDevice::new(&[], output.join("gpu"))?;
    ensure!(
        gpu.info().ray_tracing,
        "Fidelity proof needs ray queries; use the raster probe on non-RT hosts"
    );
    let settings = Settings {
        render_size: size,
        display_size: size,
        tier: Tier::Rt,
        mode: Mode::Reference {
            samples: 8,
            bounces: 1,
        },
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 1.,
        post: PostSettings {
            ev100: Some(8.),
            bloom: 0.,
            fog_density: 0.,
            aperture: 0.,
            shutter: 0.,
            ..Default::default()
        },
    };
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 50_f32.to_radians(),
    };
    let mut view = FidelityView::new(&mut gpu, &scene(), settings, None)?;
    let mut frame = 0_u64;
    loop {
        let applied = patches.frame_boundary();
        view.render(
            &mut gpu,
            &FrameInput {
                camera: &camera,
                tick: 0,
                delta_seconds: 1. / 60.,
                reset: true,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
        frame += 1;
        if frame == 1 || applied {
            view.capture(&mut gpu)?
                .save(output.join(format!("generation-{}.png", patches.status().generation)))?;
            println!(
                "ARA_FIDELITY_PATCH_FRAME pid={} frame={frame} generation={} elapsed_ms={}",
                std::process::id(),
                patches.status().generation,
                began.elapsed().as_millis()
            );
        }
        std::fs::write(
            output.join("status.json"),
            serde_json::to_vec(
                &serde_json::json!({"pid":std::process::id(),"frame":frame,"rust":patches.status()}),
            )?,
        )?;
        if output.join("stop").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    view.destroy(&mut gpu)?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}

fn scene() -> Scene {
    Scene {
        meshes: vec![Mesh {
            entity: 1,
            name: "Fidelity Rust pass proof".into(),
            vertices: [[-1., -0.8, 0.], [1., -0.8, 0.], [0., 0.8, 0.]]
                .map(|position| Vertex {
                    position,
                    normal: [0., 0., 1.],
                    uv: [0.; 2],
                    tangent: [1., 0., 0., 1.],
                })
                .to_vec(),
            indices: vec![0, 1, 2],
            material: 0,
        }],
        materials: vec![Material {
            base_color: [0.15, 0.4, 0.8, 1.],
            ..Material::default()
        }],
        textures: vec![
            Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: false,
            },
            Texture {
                size: [1, 1],
                rgba: vec![128, 128, 255, 255],
                srgb: false,
            },
        ],
        lights: Vec::new(),
        sun_direction: Vec3::new(0.3, 0.7, 0.6).normalize(),
        sun_lux: Vec3::splat(3500.),
        environment: vec![[50., 75., 100., 1.]; 128],
        environment_size: [16, 8],
    }
}
