//! Bounded thumbnail queue. Source decoding, Vulkan capture and cache IO stay off-frame.
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    scene::{self, CameraSpec, Material, Scene, Texture},
};
use glam::Vec3;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
};

pub(crate) struct Thumbnail {
    pub key: String,
    pub image: image::RgbaImage,
}
struct Request {
    path: String,
    key: String,
    source: Option<Vec<u8>>,
    plane: bool,
}
pub(crate) struct Thumbnails {
    send: SyncSender<Request>,
    receive: Receiver<Result<Thumbnail, String>>,
    requested: BTreeSet<String>,
}
impl Thumbnails {
    pub fn new(root: &Path) -> Self {
        let root = root.to_owned();
        let (send, rx) = mpsc::sync_channel::<Request>(48);
        let (tx, receive) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut renderer = None;
            for request in rx {
                let result = make(
                    &root,
                    &request.path,
                    request.source.as_deref(),
                    request.plane,
                    &mut renderer,
                )
                .map(|image| Thumbnail {
                    key: request.key,
                    image,
                })
                .map_err(|e| e.to_string());
                if tx.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            send,
            receive,
            requested: BTreeSet::new(),
        }
    }
    pub fn request(&mut self, path: &str) {
        if self.requested.contains(path) {
            return;
        }
        if self
            .send
            .try_send(Request {
                path: path.into(),
                key: path.into(),
                source: None,
                plane: false,
            })
            .is_ok()
        {
            self.requested.insert(path.to_owned());
        }
    }
    pub fn request_live(&mut self, path: &str, source: Vec<u8>, plane: bool) -> String {
        let key = format!("live:{}:{}", ara_asset::Digest::of(&source), plane);
        if !self.requested.contains(&key)
            && self
                .send
                .try_send(Request {
                    path: path.into(),
                    key: key.clone(),
                    source: Some(source),
                    plane,
                })
                .is_ok()
        {
            self.requested.insert(key.clone());
        }
        key
    }
    pub fn poll(&mut self) -> impl Iterator<Item = Result<Thumbnail, String>> + '_ {
        self.receive.try_iter().take(8)
    }
    pub fn invalidate(&mut self, path: &str) {
        self.requested.remove(path);
    }
}
fn make(
    root: &Path,
    path: &str,
    override_source: Option<&[u8]>,
    plane: bool,
    renderer: &mut Option<Renderer>,
) -> anyhow::Result<image::RgbaImage> {
    let source = root.join(path).canonicalize()?;
    anyhow::ensure!(
        source.starts_with(root.canonicalize()?),
        "thumbnail source escapes project"
    );
    let bytes = if let Some(source) = override_source {
        source.to_vec()
    } else {
        std::fs::read(&source)?
    };
    let source_len = bytes.len();
    let mut inputs = bytes;
    if let Ok(settings) = std::fs::read(root.join(format!("{path}.import.json"))) {
        inputs.extend(settings);
    }
    inputs.extend(b"ara-thumb-v2-384");
    inputs.push(u8::from(plane));
    let key = ara_asset::Digest::of(&inputs).to_string();
    let cache = root.join(".ara/cache/thumbnails");
    std::fs::create_dir_all(&cache)?;
    let target = cache.join(format!("{key}.png"));
    if target.exists() {
        return Ok(image::open(target)?.to_rgba8());
    }
    let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("");
    let image = if matches!(extension, "png" | "jpg" | "jpeg" | "hdr" | "exr") {
        let image = image::open(&source)?;
        image.thumbnail(384, 384).to_rgba8()
    } else {
        let mut scene = preview_scene();
        match extension {
            "glb" | "gltf" => {
                scene
                    .meshes
                    .push(crate::world::gltf::load(&source, 1, path.into(), 0)?)
            }
            "material" | "graph" => {
                let text = std::str::from_utf8(&inputs[..source_len])?;
                // A source material's checked constant outputs feed the renderer's PBR block.
                let material = if extension == "material" {
                    ara_material::dsl::parse(text, "gr_preview")
                        .map_err(|d| anyhow::anyhow!("{d:?}"))?
                } else {
                    let mut db = ara_schema::SchemaDb::new();
                    ara_material::register(&mut db).map_err(|d| anyhow::anyhow!(d.to_string()))?;
                    let graph =
                        ara_graph::Graph::read(text, &db).map_err(|d| anyhow::anyhow!("{d:?}"))?;
                    ara_material::Material::from_graph(&graph)
                        .map_err(|d| anyhow::anyhow!("{d:?}"))?
                };
                scene.materials[0] = material_parameters(&material)?;
                if plane {
                    scene.meshes.push(plane_mesh());
                } else {
                    scene
                        .meshes
                        .push(scene::sphere(1, path.into(), Vec3::ZERO, 1., 0));
                }
            }
            _ => anyhow::bail!("no thumbnail decoder for {extension}"),
        }
        if let Some(renderer) = renderer {
            renderer.update_geometry(scene)?;
        } else {
            *renderer = Some(Renderer::new(
                scene,
                [384, 384],
                &[],
                root.join(".ara/cache/thumbnail-gpu"),
            )?);
        }
        let renderer = renderer
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("thumbnail renderer missing"))?;
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        // Geometry bounds are provided by the renderer's stable extraction metadata.
        // Mesh source bounds are cheap to read on this worker, never in a UI frame.
        if matches!(extension, "glb" | "gltf") {
            let mesh = crate::world::gltf::load(&source, 1, path.into(), 0)?;
            for vertex in &mesh.vertices {
                min = min.min(vertex.position.into());
                max = max.max(vertex.position.into());
            }
        } else {
            min = Vec3::splat(-1.);
            max = Vec3::ONE;
        }
        renderer
            .capture(&CaptureRequest {
                camera: CameraSpec::FrameBounds {
                    min,
                    max,
                    direction: Vec3::new(1., 0.65, 1.6),
                    padding: 0.12,
                    vertical_fov: 0.8,
                },
                size: [384, 384],
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 0.0015,
                settle_frames: 4,
                tick: 0,
                seed: 0,
                marks: None,
            })?
            .image
    };
    image.save(&target)?;
    Ok(image)
}
pub(crate) fn preview_scene() -> Scene {
    Scene {
        meshes: Vec::new(),
        materials: vec![Material {
            base_color: [0.35, 0.48, 0.65, 1.],
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
        lights: Vec::new(),
        sun_direction: Vec3::new(-0.6, 1., 0.8).normalize(),
        sun_lux: Vec3::splat(3500.),
        environment: Vec::new(),
        environment_size: [0; 2],
    }
}
pub(crate) fn material_parameters(material: &ara_material::Material) -> anyhow::Result<Material> {
    let material = ara_material::fold_constants(material).map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let mut result = Material::default();
    for (output, id) in &material.outputs {
        let values = material
            .bindings
            .get(id)
            .and_then(|b| {
                if let ara_material::Expr::Literal { values } = &b.expr {
                    Some(values.as_slice())
                } else {
                    None
                }
            })
            .or_else(|| {
                material.parameters.get(id).and_then(|p| {
                    if let ara_material::ParameterValue::Numeric(v) = &p.default {
                        Some(v.as_slice())
                    } else {
                        None
                    }
                })
            });
        if let Some(v) = values {
            match output.as_str() {
                "baseColor" => {
                    for (to, from) in result.base_color.iter_mut().zip(v) {
                        *to = *from as f32;
                    }
                }
                "roughness" => result.emissive_roughness[3] = v[0] as f32,
                "metallic" => result.surface[0] = v[0] as f32,
                "emissive" => {
                    for (to, from) in result.emissive_roughness[..3].iter_mut().zip(v) {
                        *to = *from as f32;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(result)
}
/// Stable image resource identity; thumbnail resources use a bounded rotating slot pool.
pub(crate) fn image_name(slot: usize) -> String {
    format!("asset-thumbnail-{slot}")
}

fn plane_mesh() -> scene::Mesh {
    scene::Mesh {
        entity: 1,
        name: "Material plane".into(),
        material: 0,
        vertices: [[-1., -1., 0.], [1., -1., 0.], [1., 1., 0.], [-1., 1., 0.]]
            .into_iter()
            .enumerate()
            .map(|(i, position)| scene::Vertex {
                position,
                normal: [0., 0., 1.],
                uv: [[0., 1.], [1., 1.], [1., 0.], [0., 0.]][i],
                tangent: [1., 0., 0., 1.],
            })
            .collect(),
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}
