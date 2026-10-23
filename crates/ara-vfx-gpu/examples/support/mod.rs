//! Deterministic gallery scene shared by the capture tool and golden tests.
use anyhow::Result;
use ara_gpu::vulkan::VulkanDevice;
use ara_render::{
    capture::{Capture, CaptureRequest, FeatureCapture, ShaderMetadata, ViewMode},
    scene::{CameraSpec, Light, Material, Mesh, Scene, Texture, Vertex, sphere},
};
use ara_render_fidelity::{
    FidelityView, FrameInput, ImageInput, Mode, PostSettings, Settings, Tier, Upscaler,
};
use ara_vfx::{Effect, Module};
use ara_vfx_gpu::{GpuSystem, Lighting, ProbeLighting, SceneColor, SceneDepth, View};
use glam::Vec3;
use std::path::Path;

fn box_mesh(id: u64, centre: Vec3, half: Vec3, material: usize) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (n, u, v) in [
        (Vec3::X, -Vec3::Z, Vec3::Y),
        (-Vec3::X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, -Vec3::Z),
        (-Vec3::Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (-Vec3::Z, -Vec3::X, Vec3::Y),
    ] {
        let base = vertices.len() as u32;
        for (x, y) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
            vertices.push(Vertex {
                position: (centre + (n + u * x + v * y) * half).to_array(),
                normal: n.to_array(),
                uv: [(x + 1.) * 0.5, (y + 1.) * 0.5],
                tangent: u.extend(1.).to_array(),
            });
        }
        indices.extend([base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    }
    Mesh {
        entity: id,
        name: format!("gallery_{id}"),
        vertices,
        indices,
        material,
    }
}
fn gallery(effect: &Effect, tick: u32) -> Scene {
    let mut stone = Vec::new();
    for y in 0..128u32 {
        for x in 0..128u32 {
            let grain =
                ((x.wrapping_mul(73) ^ y.wrapping_mul(151) ^ x.wrapping_mul(y)) % 13) as f32;
            let a = x as f32 * std::f32::consts::TAU / 128.;
            let b = y as f32 * std::f32::consts::TAU / 128.;
            let base = (103. + grain + 10. * (a.sin() * b.cos()) + 5. * (a * 2. + b * 3.).cos())
                .clamp(0., 252.) as u8;
            stone.extend([base, base, base + 3, 255]);
        }
    }
    let mut ground = Material {
        base_color: [0.4, 0.43, 0.48, 1.],
        emissive_roughness: [0., 0., 0., 0.65],
        textures: [2, 1, 0, 0],
        ..Material::default()
    };
    if effect.name == "rain" {
        ground.base_color = [0.25, 0.28, 0.32, 1.];
        ground.emissive_roughness[3] = 0.35;
    }
    let bronze = Material {
        base_color: [0.32, 0.15, 0.055, 1.],
        emissive_roughness: [0., 0., 0., 0.3],
        surface: [0.65, 1., 1., 0.5],
        ..Material::default()
    };
    let charcoal = Material {
        base_color: [0.045, 0.025, 0.015, 1.],
        emissive_roughness: [0.03, 0.006, 0., 0.95],
        ..Material::default()
    };
    let pale = Material {
        base_color: [0.27, 0.3, 0.34, 1.],
        emissive_roughness: [0., 0., 0., 0.8],
        ..Material::default()
    };
    let mut floor = box_mesh(1, Vec3::new(0., -0.12, 0.), Vec3::new(6., 0.12, 6.), 0);
    for v in &mut floor.vertices {
        v.uv[0] *= 6.;
        v.uv[1] *= 6.;
    }
    let mut meshes = vec![
        floor,
        sphere(
            2,
            "bronze vessel".into(),
            Vec3::new(-1.55, 0.7, -0.3),
            0.55,
            1,
        ),
        box_mesh(
            3,
            Vec3::new(-1.55, 0.15, -0.3),
            Vec3::new(0.65, 0.15, 0.65),
            3,
        ),
        box_mesh(4, Vec3::new(2.15, 1.2, -2.5), Vec3::new(0.3, 1.2, 0.3), 3),
    ];
    if effect.name == "campfire" {
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::TAU / 8.;
            meshes.push(sphere(
                10 + i,
                "hearth stone".into(),
                Vec3::new(a.cos() * 0.53, 0.12, a.sin() * 0.53),
                0.17,
                3,
            ));
        }
        meshes.push(box_mesh(
            30,
            Vec3::new(0., 0.1, 0.),
            Vec3::new(0.4, 0.07, 0.1),
            2,
        ));
        meshes.push(box_mesh(
            31,
            Vec3::new(0., 0.2, 0.),
            Vec3::new(0.1, 0.07, 0.4),
            2,
        ));
    }
    if effect.name == "muzzle_flash" {
        meshes.push(box_mesh(
            40,
            Vec3::new(-0.35, 0.55, 0.),
            Vec3::new(0.4, 0.06, 0.09),
            2,
        ));
        meshes.push(box_mesh(
            41,
            Vec3::new(-0.55, 0.42, 0.),
            Vec3::new(0.08, 0.17, 0.07),
            2,
        ));
    }
    let mut lights = vec![Light {
        position_range: [-2., 3., 2., 8.],
        color_lumens: [0.5, 0.65, 1., 180.],
        direction_outer: [0.; 4],
        parameters: [0.; 4],
    }];
    let flicker = 0.85 + 0.1 * (tick as f32 * 0.73).sin() + 0.05 * (tick as f32 * 1.37).sin();
    for e in &effect.emitters {
        for m in &e.stacks.render {
            if let Module::Light {
                color,
                lumens,
                radius,
            } = m.module
            {
                if lights.len() <= effect.quality.lights as usize {
                    lights.push(Light {
                        position_range: [
                            (e.bounds.min[0] + e.bounds.max[0]) * 0.5,
                            0.7,
                            (e.bounds.min[2] + e.bounds.max[2]) * 0.5,
                            radius,
                        ],
                        color_lumens: [
                            color[0],
                            color[1],
                            color[2],
                            lumens
                                * flicker
                                * if effect.name == "muzzle_flash" {
                                    (-(tick as f32) * 0.2).exp()
                                } else {
                                    1.
                                },
                        ],
                        direction_outer: [0.; 4],
                        parameters: [0.; 4],
                    });
                }
            }
        }
    }
    Scene {
        meshes,
        materials: vec![ground, bronze, charcoal, pale],
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
            Texture {
                size: [128, 128],
                rgba: stone,
                srgb: true,
            },
        ],
        lights,
        sun_direction: Vec3::new(-0.5, 1., 0.3).normalize(),
        sun_lux: Vec3::new(2.8, 3.2, 4.),
        environment: vec![[0.15, 0.2, 0.3, 1.]],
        environment_size: [1, 1],
    }
}
fn camera(effect: &Effect) -> CameraSpec {
    let position = if effect.name == "smoke" {
        Vec3::new(5., 3.4, 6.)
    } else {
        Vec3::new(4.2, 2.8, 5.)
    };
    CameraSpec::LookAt {
        position,
        target: Vec3::new(0., 1.2, 0.),
        vertical_fov: 0.7,
    }
}
/// Four representative fixed ticks, including the sample's mid-life golden.
pub fn sequence_ticks(name: &str) -> [u32; 4] {
    match name {
        "muzzle_flash" => [1, 2, 3, 6],
        "campfire" => [30, 70, 120, 180],
        "sparks" => [10, 40, 100, 160],
        "magic_swirl" => [30, 75, 140, 200],
        "smoke" => [40, 100, 200, 280],
        _ => [30, 60, 100, 160],
    }
}
/// Four independently replayed frames in chronological order, at half size.
pub fn sequence_strip(frames: &[(Capture, ara_vfx_gpu::Stats)]) -> image::RgbaImage {
    let mut strip = image::RgbaImage::new(1920, 270);
    for (i, (frame, _)) in frames.iter().take(4).enumerate() {
        let thumbnail = image::imageops::resize(
            &frame.image,
            480,
            270,
            image::imageops::FilterType::Lanczos3,
        );
        image::imageops::replace(&mut strip, &thumbnail, (i * 480) as i64, 0);
    }
    strip
}
/// Replay independently at every requested tick, then converge the scene with a
/// fixed 64-frame probe/lighting sequence and draw the frozen VFX before bloom.
pub fn capture_frames(
    gpu: &mut VulkanDevice,
    effect: &Effect,
    ticks: &[u32],
    size: [u32; 2],
) -> Result<Vec<(Capture, ara_vfx_gpu::Stats)>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut captures = Vec::new();
    for &tick in ticks {
        let mut system =
            GpuSystem::new(gpu, effect, &root.join("cooked").join(&effect.name), size)?;
        let camera = camera(effect);
        let request = CaptureRequest {
            camera: camera.clone(),
            size,
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: 1.,
            settle_frames: 1,
            tick: u64::from(tick),
            seed: u64::from(effect.seed),
            marks: Some(1),
        };
        system.capture(gpu, &request)?;
        let scene = gallery(effect, tick);
        let settings = Settings {
            render_size: size,
            display_size: size,
            tier: Tier::Rt,
            mode: Mode::Hybrid,
            upscaler: Upscaler::Taau,
            probe_ray_budget: 38400,
            reflection_cutoff: 0.6,
            post: PostSettings {
                ev100: Some(0.),
                bloom: 0.12,
                fog_density: 0.001,
                ..PostSettings::default()
            },
        };
        let mut fidelity = FidelityView::new(gpu, &scene, settings, None)?;
        for frame in 0..64 {
            fidelity.render(
                gpu,
                &FrameInput {
                    camera: &camera,
                    tick: frame,
                    delta_seconds: 1. / 60.,
                    reset: frame == 0,
                    lighting_changed: frame == 0,
                    deterministic: true,
                },
            )?;
        }
        let inputs = fidelity.feature_inputs()?;
        let view = View {
            camera: camera.clone(),
            scene_background: Some(SceneColor {
                image: inputs.radiance.image,
                sampled_index: inputs.radiance.sampled_index,
                state: inputs.radiance.state,
            }),
            scene_depth: Some(SceneDepth {
                image: inputs.depth.image,
                sampled_index: inputs.depth.sampled_index,
                state: inputs.depth.state,
            }),
            linear_depth: true,
            lighting: Lighting {
                ambient: Vec3::new(0.15, 0.2, 0.3),
                sun_direction: scene.sun_direction,
                sun_color: scene.sun_lux,
                probes: Some(ProbeLighting {
                    buffer: inputs.probes,
                    index: inputs.probe_index,
                    origin: inputs.volume.origin,
                    spacing: inputs.volume.spacing,
                    counts: inputs.volume.counts,
                    state: inputs.probe_state,
                }),
            },
            ..View::default()
        };
        system.render(gpu, &view)?;
        let hdr = system.hdr_output();
        fidelity.post_process(
            gpu,
            &FrameInput {
                camera: &camera,
                tick,
                delta_seconds: 1. / 60.,
                reset: true,
                lighting_changed: false,
                deterministic: true,
            },
            ImageInput {
                image: hdr.image,
                sampled_index: hdr.sampled_index,
                state: hdr.state,
            },
        )?;
        let mut timings = system.stats(gpu)?.gpu_stages;
        timings.extend_from_slice(fidelity.timings());
        let particles: ara_gpu::cooked::CookedPipeline = serde_json::from_slice(&std::fs::read(
            root.join("cooked")
                .join(&effect.name)
                .join("particles.json"),
        )?)?;
        let post: ara_gpu::cooked::CookedPipeline = serde_json::from_slice(&std::fs::read(
            root.join("../ara-render-fidelity/cooked/post.json"),
        )?)?;
        let combined = blake3::hash(
            format!(
                "{}:{}:{}",
                particles.key,
                post.key,
                blake3::hash(&std::fs::read(root.join("cooked/atlas.rgba"))?)
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        let shaders = ShaderMetadata {
            content_key: combined,
            compiler: particles.compiler,
            binding_layout_hash: particles.binding_layout_hash,
        };
        let hash = blake3::hash(
            format!(
                "{}:{}:{settings:?}",
                effect
                    .content_hash()
                    .map_err(|d| anyhow::anyhow!("{d:?}"))?,
                fidelity.scene().content_hash()
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        let names = vec![(1, effect.name.clone())];
        let capture = Capture::from_feature(
            FeatureCapture {
                rgba: fidelity.capture(gpu)?.into_raw(),
                ids: system.read_coverage(gpu)?,
                names: &names,
                hash: &hash,
                device: &gpu.context().info,
                timings: &timings,
                shaders: &shaders,
            },
            &request,
        )?;
        let mut stats = system.stats(gpu)?;
        stats.gpu_stages.extend_from_slice(fidelity.timings());
        captures.push((capture, stats));
        fidelity.destroy(gpu)?;
        system.retire(gpu)?;
    }
    gpu.context().assert_clean()?;
    Ok(captures)
}
