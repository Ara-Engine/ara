//! Repeated shared-GPU motion measurements: no argument uses the legacy path;
//! `native` moves an instance, and `native-skin` updates one independent skin.
use ara_gpu::Device;
use ara_gpu::GpuDevice;
use ara_render::scene::{CameraSpec, Material, Mesh, Scene, Texture, Vertex};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use std::{path::Path, time::Instant};

const COUNT: usize = 2048;
const ITERATIONS: u32 = 30;
const QUAD: [[f32; 3]; 4] = [
    [-0.08, -0.08, 0.0],
    [0.08, -0.08, 0.0],
    [0.08, 0.08, 0.0],
    [-0.08, 0.08, 0.0],
];

fn centre(index: usize) -> [f32; 3] {
    [
        (index % 48) as f32 * 0.25 - 6.0,
        (index / 48) as f32 * 0.25 - 5.0,
        0.0,
    ]
}
fn legacy() -> Scene {
    Scene {
        meshes: (0..COUNT)
            .map(|index| {
                let c = centre(index);
                Mesh {
                    entity: index as u64,
                    name: format!("actor{index}"),
                    vertices: QUAD
                        .map(|p| Vertex {
                            position: [p[0] + c[0], p[1] + c[1], p[2]],
                            normal: [0.0, 0.0, 1.0],
                            uv: [0.0; 2],
                            tangent: [1.0, 0.0, 0.0, 1.0],
                        })
                        .to_vec(),
                    indices: vec![0, 1, 2, 0, 2, 3],
                    material: 0,
                }
            })
            .collect(),
        materials: vec![Material::default()],
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
        sun_direction: [0.0, 0.0, 1.0].into(),
        sun_lux: [0.0; 3].into(),
        environment: vec![[0.0, 0.0, 0.0, 1.0]],
        environment_size: [1, 1],
    }
}
fn quantile(values: &mut [f64], percent: usize) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() * percent / 100).min(values.len() - 1)]
}
fn native(skin: bool) -> ara_render::instanced::Scene {
    use ara_render::instanced::{Geometry, Instance, Lod, Material, Scene, Vertex};
    Scene {
        geometry: (0..if skin { 2 } else { 1 })
            .map(|index| Geometry {
                key: [index + 1; 32],
                vertices: QUAD
                    .map(|position| Vertex {
                        position,
                        normal: [0.0, 0.0, 1.0],
                        uv: [0.0; 2],
                        uv1: [0.0; 2],
                        tangent: [1.0, 0.0, 0.0, 1.0],
                        terrain_weights: 0,
                        reserved: 0,
                    })
                    .to_vec(),
                lods: vec![Lod {
                    indices: vec![0, 1, 2, 0, 2, 3],
                    error: 0.0,
                }],
                bounds: [[-0.08, -0.08, 0.0].into(), [0.08, 0.08, 0.0].into()],
                deforming: index == 1,
                revision: 0,
            })
            .collect(),
        instances: (0..COUNT)
            .map(|index| {
                let c = centre(index);
                let mut instance = Instance {
                    entity: index as u64,
                    name: format!("actor{index}"),
                    geometry: usize::from(skin && index == COUNT / 2),
                    material: 0,
                    world: Default::default(),
                    previous_world: Default::default(),
                };
                instance.world.w_axis.x = c[0];
                instance.world.w_axis.y = c[1];
                instance.previous_world = instance.world;
                instance
            })
            .collect(),
        materials: vec![Material::default()],
        textures: 0,
        lights: Vec::new(),
        sun_direction: [0.0, 0.0, 1.0].into(),
        sun_lux: [0.0; 3].into(),
        environment: vec![[0.0, 0.0, 0.0, 1.0]],
        environment_size: [1, 1],
    }
}
fn main() -> anyhow::Result<()> {
    let output = Path::new("target/renderer-unification/checks");
    let mut gpu = GpuDevice::new(&[], "target/renderer-unification/native-instancing/cache")?;
    let mut scene = legacy();
    let mode = std::env::args().nth(1);
    let is_skin = mode.as_deref() == Some("native-skin");
    let is_native = is_skin || mode.as_deref() == Some("native");
    let mut native_scene = native(is_skin);
    let settings = Settings {
        render_size: [128, 64],
        display_size: [128, 64],
        tier: Tier::Rt,
        mode: Mode::Reference {
            samples: 8,
            bounces: 1,
        },
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 1.0,
        post: PostSettings {
            bloom: 0.0,
            fog_density: 0.0,
            aperture: 0.0,
            shutter: 0.0,
            ..Default::default()
        },
    };
    let camera = CameraSpec::LookAt {
        position: [0.0, 0.0, 3.0].into(),
        target: [0.0; 3].into(),
        vertical_fov: 60.0f32.to_radians(),
    };
    let created = Instant::now();
    let mut view = if is_native {
        FidelityView::new_native(
            &mut gpu,
            &native_scene,
            ara_texture_stream::NativeResidency::new(ara_texture_stream::NativeConfig {
                resident_bytes: 1 << 20,
                upload_bytes: 1 << 20,
            })?,
            settings,
            None,
        )?
    } else {
        FidelityView::new(&mut gpu, &scene, settings, None)?
    };
    let create_ms = created.elapsed().as_secs_f64() * 1000.0;
    let mut prepare = Vec::new();
    let mut render = Vec::new();
    let mut gpu_prepare = Vec::new();
    let mut gpu_total = Vec::new();
    for step in 0..ITERATIONS + 8 {
        let c = centre(COUNT / 2);
        let offset = (step % 3) as f32 * 0.02;
        for (v, p) in scene.meshes[COUNT / 2].vertices.iter_mut().zip(QUAD) {
            v.position = [p[0] + c[0] + offset, p[1] + c[1], p[2]];
        }
        if is_skin {
            let geometry = &mut native_scene.geometry[1];
            for (vertex, source) in geometry.vertices.iter_mut().zip(QUAD) {
                vertex.position[0] = source[0] + offset;
            }
            geometry.bounds[0].x = -0.08 + offset;
            geometry.bounds[1].x = 0.08 + offset;
            geometry.revision += 1;
        } else {
            native_scene.instances[COUNT / 2].world.w_axis.x = c[0] + offset;
        }
        let started = Instant::now();
        let gpu_ms = if is_native {
            view.update_native_scene(&mut gpu, &native_scene)?.gpu_ms
        } else {
            view.update_scene(&mut gpu, &scene)?;
            0.0
        };
        let update_ms = started.elapsed().as_secs_f64() * 1000.0;
        view.render(
            &mut gpu,
            &FrameInput {
                camera: &camera,
                tick: step,
                delta_seconds: 1.0 / 60.0,
                reset: true,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
        if step >= 8 {
            prepare.push(update_ms);
            let render_ms = view.timings().iter().map(|t| t.milliseconds).sum::<f64>();
            render.push(render_ms);
            gpu_prepare.push(gpu_ms);
            gpu_total.push(gpu_ms + render_ms);
        }
    }
    let mut result = std::collections::BTreeMap::from([
        ("instances", COUNT as f64),
        (
            "uniqueGpuGeometry",
            if is_native {
                native_scene.geometry.len() as f64
            } else {
                COUNT as f64
            },
        ),
        ("iterations", ITERATIONS as f64),
        ("width", 128.0),
        ("height", 64.0),
        ("createMs", create_ms),
        ("prepareWallP50Ms", quantile(&mut prepare, 50)),
        ("prepareWallP95Ms", quantile(&mut prepare, 95)),
        ("renderGpuP50Ms", quantile(&mut render, 50)),
        ("renderGpuP95Ms", quantile(&mut render, 95)),
    ]);
    if is_native {
        result.insert("prepareGpuP50Ms", quantile(&mut gpu_prepare, 50));
        result.insert("prepareGpuP95Ms", quantile(&mut gpu_prepare, 95));
        result.insert("totalGpuP50Ms", quantile(&mut gpu_total, 50));
        result.insert("totalGpuP95Ms", quantile(&mut gpu_total, 95));
    }
    ara_gpu::cooked::write_json(
        &output.join(if is_skin {
            "motion-native-skin-after.json"
        } else if is_native {
            "motion-native-after.json"
        } else {
            "motion-legacy-before.json"
        }),
        &result,
    )?;
    println!("{result:?}");
    view.destroy(&mut gpu)?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
