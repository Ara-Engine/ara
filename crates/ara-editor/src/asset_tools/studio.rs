//! Studio shots for model thumbnails (the UE Content Browser / Unity Project look).
//! Models keep their own materials; every subject is normalized to a unit bounding
//! sphere, framed tightly from a front-right 3/4 view, lit by soft studio softboxes
//! and composited over one neutral backdrop, so the grid reads as a consistent set.
//! Shots render at [`MASTER`] pixels; `thumbnails` resamples them per view.
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    scene::{CameraSpec, Material, Mesh, Scene, Texture, Vertex},
};
use glam::Vec3;
use image::{Rgba, RgbaImage};
use std::{collections::BTreeMap, path::Path};

/// Rendered shot edge in pixels. Views receive a Lanczos resample at 2× their
/// logical size, so 1× displays an exact 2:1 reduction and 2× a 1:1 image.
pub(crate) const MASTER: u32 = 512;
/// Largest uploaded texture edge: the renderer samples a single mip, so larger
/// sources would alias in a 512 px shot.
const TEXTURE_EDGE: u32 = 512;
/// Camera bearing from the front (+Z) toward the right (+X), and its height.
const AZIMUTH: f32 = 0.62;
const ELEVATION: f32 = 0.42;
/// A long lens keeps proportions honest; the subject fills all but this margin.
const FOV: f32 = 0.5;
const MARGIN: f32 = 0.1;
/// Key light, toward the light: above and to the left of the camera.
const KEY: Vec3 = Vec3::new(-0.38, 0.74, 0.56);
const KEY_LUX: f32 = 1.8;
const EXPOSURE: f32 = 0.8;
/// Procedural studio environment resolution (equirectangular).
const ENVIRONMENT: [u32; 2] = [128, 64];

/// Geometry and surfaces to photograph, in model space.
pub(super) struct Subject {
    meshes: Vec<Mesh>,
    materials: Vec<Material>,
    textures: Vec<Texture>,
}

impl Subject {
    /// A published glTF generation with its own materials and decoded textures.
    pub fn model(model: &ara_runtime::model::Model) -> anyhow::Result<Self> {
        let mut subject = Self::clay(Vec::new());
        let mut slots = BTreeMap::<usize, u32>::new();
        for source in &model.materials {
            let mut material = Material {
                base_color: source.base_color,
                emissive_roughness: [
                    source.emissive[0],
                    source.emissive[1],
                    source.emissive[2],
                    source.roughness,
                ],
                surface: [
                    source.metallic,
                    source.normal_scale,
                    1.,
                    source.alpha_cutoff,
                ],
                ..Default::default()
            };
            material.flags[0] = source.alpha_mode;
            for (slot, texture) in [
                source.base_color_texture,
                source.normal_texture,
                source.metallic_roughness_texture,
                source.emissive_texture,
            ]
            .into_iter()
            .enumerate()
            {
                let Some(index) = texture else {
                    continue;
                };
                material.textures[slot] = if let Some(slot) = slots.get(&index) {
                    *slot
                } else {
                    let decoded = ara_runtime_render::decode_texture(&model.textures[index].ktx2)?;
                    let slot = u32::try_from(subject.textures.len())?;
                    subject.textures.push(shrink(decoded));
                    slots.insert(index, slot);
                    slot
                };
            }
            subject.materials.push(material);
        }
        for (n, primitive) in model.primitives.iter().enumerate() {
            subject.meshes.push(Mesh {
                entity: n as u64 + 1,
                name: format!("primitive {n}"),
                vertices: primitive
                    .vertices
                    .iter()
                    .map(|v| Vertex {
                        position: v.position,
                        normal: v.normal,
                        uv: v.uv,
                        tangent: v.tangent,
                    })
                    .collect(),
                indices: primitive.indices.clone(),
                // Material 0 is the clay fallback for primitives without one.
                material: primitive.material.map_or(0, |m| m + 1),
            });
        }
        Ok(subject)
    }

    /// Untextured geometry (FBX meshes use material 0) in a neutral clay.
    pub fn clay(meshes: Vec<Mesh>) -> Self {
        Self {
            meshes,
            materials: vec![Material {
                base_color: [0.5, 0.5, 0.48, 1.],
                emissive_roughness: [0., 0., 0., 0.6],
                ..Default::default()
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
        }
    }
}

/// Photograph a subject: normalize, frame, light, render and composite the backdrop.
/// `renderer` is the worker's reusable raster renderer (created on first use).
pub(super) fn shoot(
    mut subject: Subject,
    renderer: &mut Option<Renderer>,
    cache: &Path,
) -> anyhow::Result<RgbaImage> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in subject.meshes.iter().flat_map(|mesh| &mesh.vertices) {
        min = min.min(vertex.position.into());
        max = max.max(vertex.position.into());
    }
    anyhow::ensure!(
        min.is_finite() && max.is_finite(),
        "thumbnail mesh is empty"
    );
    // Presentation only: any model size frames the same way, clear of the near plane.
    let centre = (min + max) * 0.5;
    let scale = 1. / ((max - min).length() * 0.5).max(1e-6);
    for vertex in subject.meshes.iter_mut().flat_map(|m| &mut m.vertices) {
        vertex.position = ((Vec3::from(vertex.position) - centre) * scale).into();
    }
    let camera = frame((max - min) * 0.5 * scale);
    let scene = Scene {
        meshes: subject.meshes,
        materials: subject.materials,
        textures: subject.textures,
        lights: Vec::new(),
        sun_direction: KEY.normalize(),
        sun_lux: Vec3::new(1., 0.97, 0.92) * KEY_LUX,
        environment: environment(),
        environment_size: ENVIRONMENT,
    };
    let renderer = match renderer {
        Some(renderer) => {
            renderer.replace_scene(scene)?;
            renderer
        }
        None => renderer.insert(Renderer::new_raster(scene, [MASTER; 2], &[], cache)?),
    };
    let capture = renderer.capture(&CaptureRequest {
        camera,
        size: [MASTER; 2],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: EXPOSURE,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    })?;
    let mut image = capture.image.clone();
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        if capture.entity_at(x, y).is_none() {
            *pixel = backdrop(x, y, MASTER);
        }
    }
    Ok(image)
}

/// A front-right 3/4 view whose square frame fits the box's projected corners.
fn frame(half: Vec3) -> CameraSpec {
    let direction = Vec3::new(
        AZIMUTH.sin() * ELEVATION.cos(),
        ELEVATION.sin(),
        AZIMUTH.cos() * ELEVATION.cos(),
    );
    let forward = -direction;
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let tan = (FOV * 0.5).tan() * (1. - MARGIN);
    let corners = (0..8).map(|i| {
        Vec3::new(
            if i & 1 == 0 { -half.x } else { half.x },
            if i & 2 == 0 { -half.y } else { half.y },
            if i & 4 == 0 { -half.z } else { half.z },
        )
    });
    let mut target = Vec3::ZERO;
    let mut distance = 3.;
    // Fit the distance, then centre the projected extents; two rounds converge.
    for _ in 0..3 {
        distance = corners
            .clone()
            .map(|p| {
                let q = p - target;
                q.dot(right).abs().max(q.dot(up).abs()) / tan - q.dot(forward)
            })
            .fold(f32::MIN, f32::max);
        let eye = target + direction * distance;
        let (mut low, mut high) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in corners.clone() {
            let q = p - eye;
            let depth = q.dot(forward);
            for (axis, value) in [q.dot(right) / depth, q.dot(up) / depth]
                .into_iter()
                .enumerate()
            {
                low[axis] = low[axis].min(value);
                high[axis] = high[axis].max(value);
            }
        }
        target += (right * (low[0] + high[0]) + up * (low[1] + high[1])) * 0.5 * distance;
    }
    CameraSpec::LookAt {
        position: target + direction * distance,
        target,
        vertical_fov: FOV,
    }
}

/// Soft studio light as radiance (cd/m²): a dim cyclorama with a horizon sweep,
/// a rectangular key softbox matching the sun, a tall cool fill strip on the right
/// and a rim strip behind. `material_preview.slang` mirrors these softboxes.
fn environment() -> Vec<[f32; 4]> {
    let [width, height] = ENVIRONMENT;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for j in 0..height {
        for i in 0..width {
            let phi = ((i as f32 + 0.5) / width as f32 - 0.5) * std::f32::consts::TAU;
            let theta = (j as f32 + 0.5) / height as f32 * std::f32::consts::PI;
            let d = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            let base = 0.05 + 0.06 * d.y.max(0.) + 0.16 * (-(d.y / 0.14).powi(2)).exp();
            let mut radiance = Vec3::splat(base);
            for (centre, size, colour) in SOFTBOXES {
                radiance += Vec3::from(colour) * softbox(d, Vec3::from(centre).normalize(), size);
            }
            pixels.push([radiance.x, radiance.y, radiance.z, 1.]);
        }
    }
    pixels
}

/// Studio softboxes: centre direction, half extents (tangent of the half angles)
/// and radiance. The key sits on [`KEY`].
const SOFTBOXES: [([f32; 3], [f32; 2], [f32; 3]); 3] = [
    ([-0.38, 0.74, 0.56], [0.45, 0.3], [2.2, 2.13, 2.02]),
    ([0.95, 0.25, 0.2], [0.3, 0.6], [0.56, 0.6, 0.65]),
    ([0.1, 0.55, -0.83], [0.8, 0.12], [2.6, 2.6, 2.6]),
];

/// A soft-edged rectangle seen through a gnomonic projection around `centre`.
fn softbox(d: Vec3, centre: Vec3, size: [f32; 2]) -> f32 {
    let facing = d.dot(centre);
    if facing <= 0. {
        return 0.;
    }
    let tangent = centre.cross(Vec3::Y).normalize();
    let bitangent = tangent.cross(centre);
    let edge = |offset: f32, half: f32| {
        let t = ((half - offset.abs()) / (half * 0.3)).clamp(0., 1.);
        t * t * (3. - 2. * t)
    };
    edge(d.dot(tangent) / facing, size[0]) * edge(d.dot(bitangent) / facing, size[1])
}

/// The shared neutral backdrop: a cool grey gradient with a soft light behind the subject.
pub(super) fn backdrop(x: u32, y: u32, size: u32) -> Rgba<u8> {
    let u = (x as f32 + 0.5) / size as f32;
    let v = (y as f32 + 0.5) / size as f32;
    let glow = (1. - ((u - 0.5).powi(2) + (v - 0.42).powi(2)).sqrt() * 1.6).clamp(0., 1.);
    let shade = |top: f32, bottom: f32| {
        let value = top + (bottom - top) * v + glow * glow * 10.;
        value.round().clamp(0., 255.) as u8
    };
    Rgba([shade(60., 34.), shade(63., 36.), shade(69., 40.), 255])
}

/// Bound a decoded texture to [`TEXTURE_EDGE`] with a triangle (area) filter.
fn shrink(texture: Texture) -> Texture {
    let [width, height] = texture.size;
    let largest = width.max(height);
    if largest <= TEXTURE_EDGE {
        return texture;
    }
    let Some(image) = RgbaImage::from_raw(width, height, texture.rgba) else {
        return Texture {
            size: [1, 1],
            rgba: vec![255; 4],
            srgb: texture.srgb,
        };
    };
    let size = [
        (width * TEXTURE_EDGE / largest).max(1),
        (height * TEXTURE_EDGE / largest).max(1),
    ];
    let small = image::imageops::resize(
        &image,
        size[0],
        size[1],
        image::imageops::FilterType::Triangle,
    );
    Texture {
        size,
        rgba: small.into_raw(),
        srgb: texture.srgb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fitted camera keeps every corner inside the square frame, with the margin.
    #[test]
    fn framing_fits_tall_wide_and_flat_boxes() {
        for half in [
            Vec3::new(0.2, 0.95, 0.2),
            Vec3::new(0.95, 0.2, 0.2),
            Vec3::new(0.7, 0.05, 0.7),
        ] {
            let CameraSpec::LookAt {
                position, target, ..
            } = frame(half)
            else {
                panic!("look-at camera");
            };
            let forward = (target - position).normalize();
            let right = forward.cross(Vec3::Y).normalize();
            let up = right.cross(forward);
            let tan = (FOV * 0.5).tan();
            let mut widest: f32 = 0.;
            for i in 0..8 {
                let p = Vec3::new(
                    if i & 1 == 0 { -half.x } else { half.x },
                    if i & 2 == 0 { -half.y } else { half.y },
                    if i & 4 == 0 { -half.z } else { half.z },
                );
                let q = p - position;
                let depth = q.dot(forward);
                let x = q.dot(right) / depth / tan;
                let y = q.dot(up) / depth / tan;
                assert!(x.abs() <= 1. && y.abs() <= 1., "{half} corner {i} outside");
                widest = widest.max(x.abs()).max(y.abs());
            }
            assert!(widest > 0.85, "{half} is framed loosely ({widest})");
        }
    }

    /// Writes studio shots of the sample models to `target/thumbnail-studio`.
    /// Needs a Vulkan GPU: `cargo test -p ara-editor --lib studio -- --ignored`.
    #[test]
    #[ignore = "needs a Vulkan GPU"]
    fn studio_shots_of_sample_models() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let output = crates.join("../target/thumbnail-studio");
        std::fs::create_dir_all(&output).expect("output");
        // The shader cook takes plain absolute paths.
        let output = dunce::canonicalize(output).expect("output path");
        let registry = ara_importers::registry().expect("importers");
        let importer = registry.get("ara.gltf").expect("glTF importer");
        let mut renderer = None;
        for (name, path) in [
            ("avocado", "ara-render/examples/assets/Avocado.glb"),
            ("fox", "ara-player/demo/content/assets/fox/Fox.glb"),
            ("rock", "ara-player/demo/content/assets/props/rock.glb"),
        ] {
            let input = ara_asset::ImportInput {
                preset: ara_asset::CookPreset::Fast,
                thread_budget: 4,
                source_path: path.into(),
                source: std::fs::read(crates.join(path)).expect("model source"),
                importer: importer.id().into(),
                version: importer.version().into(),
                settings: serde_json::json!({"virtualGeometry":false}),
                target: ara_asset::TargetPlatform::Windows,
                dependencies: BTreeMap::new(),
                asset_dependencies: BTreeMap::new(),
            };
            let output_cook = registry
                .import(
                    &input,
                    &mut ara_asset::ImportContext {
                        metrics: None,
                        cancellation: &ara_asset::Cancellation::default(),
                        progress: &mut |_| {},
                    },
                )
                .expect("cook");
            let key = input.key().expect("key");
            let cached = ara_asset::CachedImport {
                key,
                output: output_cook,
            };
            let model = ara_runtime::model::Model::decode(name, key, &cached).expect("model");
            let shot = shoot(
                Subject::model(&model).expect("subject"),
                &mut renderer,
                &output.join("gpu"),
            )
            .expect("shot");
            shot.save(output.join(format!("{name}-master.png")))
                .expect("save");
            for px in [128, 256] {
                super::super::thumbnails::present(&shot, px)
                    .save(output.join(format!("{name}-{px}.png")))
                    .expect("save");
            }
        }
        let clay = shoot(
            Subject::clay(vec![capsule()]),
            &mut renderer,
            &output.join("gpu"),
        )
        .expect("clay");
        clay.save(output.join("clay-master.png")).expect("save");
        // Materials share the studio: dielectric, metal and rough rubber on the sphere.
        for (name, color, metallic, roughness) in [
            ("plastic", "0.62, 0.06, 0.05", 0., 0.35),
            ("gold", "1.0, 0.71, 0.29", 1., 0.25),
            ("rubber", "0.04, 0.04, 0.045", 0., 0.85),
        ] {
            let source = format!(
                "material Preview : surface budget 1 {{
  param color : ColorLinear = linear({color});
  let metallic : float = {metallic:.1};
  let roughness : float = {roughness};
  output baseColor = color;
  output metallic = metallic;
  output roughness = roughness;
}}
"
            );
            let material =
                ara_material::dsl::parse(&source, "gr_preview").expect("material source");
            let sphere = super::super::material_preview::capture(
                &output,
                &material,
                super::super::material_preview::Shot::surface(false),
            )
            .expect("material preview");
            sphere
                .save(output.join(format!("material-{name}-master.png")))
                .expect("save");
        }
        // In-node previews of the showcase's operations (swatches and spheres).
        let showcase = ara_material::dsl::parse(super::super::node_preview::SHOWCASE, "gr_preview")
            .expect("showcase");
        for (id, binding) in &showcase.bindings {
            if let Some((derived, shot)) = super::super::node_preview::derive(&showcase, id) {
                super::super::material_preview::capture(&output, &derived, shot)
                    .expect("node preview")
                    .save(output.join(format!("node-{}.png", binding.name)))
                    .expect("save");
            }
        }
    }

    fn capsule() -> Mesh {
        let shape = ara_runtime_render::geometry::shape(
            ara_runtime::components::MeshShape::Capsule,
            [0.6, 1.4, 0.6],
        );
        Mesh {
            entity: 1,
            name: "capsule".into(),
            vertices: shape
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    uv: v.uv,
                    tangent: v.tangent,
                })
                .collect(),
            indices: shape.indices,
            material: 0,
        }
    }
}
