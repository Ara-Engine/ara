//! Bounded thumbnail queue. Source decoding, Vulkan capture and cache IO stay off-frame.
//! Every preview renders once as a square master (models through `studio`, materials
//! on a sphere, effects at a fixed tick) and is cached by content; each view gets a
//! Lanczos resample at its pixel size with rounded corners, so the UI's single-mip
//! bilinear sampler never minifies more than 2:1.
use super::material_preview::Shot;
use super::studio::{self, MASTER};
use ara_render::Renderer;
use image::RgbaImage;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
    time::Duration,
};

/// Pixel size of Assets grid tiles: 2× their 64 px logical thumbnail.
pub(crate) const TILE: u32 = 128;
/// Longest wait for an in-flight import to publish a model before reporting it.
const PUBLISH_WAIT: Duration = Duration::from_secs(30);

pub(crate) struct Thumbnail {
    pub key: String,
    pub image: RgbaImage,
}
struct Request {
    path: String,
    key: String,
    source: Source,
    shot: Shot,
    pixels: u32,
}
/// What a request renders: the project file, unsaved bytes of it, or a derived material.
enum Source {
    File,
    Bytes(Vec<u8>),
    Material(Box<ara_material::Material>),
}
pub(crate) struct Thumbnails {
    send: SyncSender<Request>,
    /// In-node previews queue on their own worker: behind nothing, blocking nothing.
    nodes: SyncSender<Request>,
    receive: Receiver<Result<Thumbnail, (String, String)>>,
    requested: BTreeSet<String>,
}
/// Key prefix of in-node previews; their failures are not reported (the node's are).
const NODE: &str = "live:node:";
impl Thumbnails {
    pub fn new(root: &Path) -> Self {
        let (send, requests) = mpsc::sync_channel::<Request>(48);
        let (nodes, node_requests) = mpsc::sync_channel::<Request>(32);
        let (results, receive) = mpsc::sync_channel(8);
        worker(root.to_owned(), requests, results.clone());
        worker(root.to_owned(), node_requests, results);
        Self {
            send,
            nodes,
            receive,
            requested: BTreeSet::new(),
        }
    }
    /// An Assets grid tile, keyed by its project path.
    pub fn request(&mut self, path: &str) {
        self.enqueue(path, path.into(), Source::File, Shot::surface(false), TILE);
    }
    /// A larger view of a project asset (import preview), `pixels` square; returns
    /// its image key. Like live previews it survives browser refreshes as last good.
    pub fn request_sized(&mut self, path: &str, pixels: u32) -> String {
        let key = format!("live:{pixels}:{path}");
        self.enqueue(
            path,
            key.clone(),
            Source::File,
            Shot::surface(false),
            pixels,
        );
        key
    }
    /// A preview of unsaved source (the material editor's live render); returns its key.
    pub fn request_live(
        &mut self,
        path: &str,
        source: Vec<u8>,
        plane: bool,
        pixels: u32,
    ) -> String {
        let key = format!("live:{}:{plane}:{pixels}", ara_asset::Digest::of(&source));
        self.enqueue(
            path,
            key.clone(),
            Source::Bytes(source),
            Shot::surface(plane),
            pixels,
        );
        key
    }
    /// The key of a derived material's preview (a graph node's value); identical
    /// previews share it.
    pub fn material_key(material: &ara_material::Material, shot: Shot, pixels: u32) -> String {
        let text = ara_material::dsl::print(material).unwrap_or_default();
        format!(
            "{NODE}{}:{}:{pixels}",
            ara_asset::Digest::of(text.as_bytes()),
            shot.tag()
        )
    }
    /// Queue a derived material's preview under its `material_key`; `path` is the
    /// material it came from. A full queue drops it: call again to retry.
    pub fn request_material(
        &mut self,
        path: &str,
        key: &str,
        material: &ara_material::Material,
        shot: Shot,
        pixels: u32,
    ) {
        if !self.requested.contains(key) {
            self.enqueue(
                path,
                key.into(),
                Source::Material(Box::new(material.clone())),
                shot,
                pixels,
            );
        }
    }
    fn enqueue(&mut self, path: &str, key: String, source: Source, shot: Shot, pixels: u32) {
        if self.requested.contains(&key) {
            return;
        }
        let request = Request {
            path: path.into(),
            key: key.clone(),
            source,
            shot,
            pixels: pixels.clamp(16, MASTER),
        };
        let queue = if matches!(request.source, Source::Material(_)) {
            &self.nodes
        } else {
            &self.send
        };
        if queue.try_send(request).is_ok() {
            self.requested.insert(key);
        }
    }
    pub fn poll(&mut self) -> impl Iterator<Item = Result<Thumbnail, (String, String)>> + '_ {
        self.receive
            .try_iter()
            .take(8)
            .filter(|result| !matches!(result, Err((key, _)) if key.starts_with(NODE)))
    }
    pub fn invalidate_all(&mut self) {
        self.requested.clear();
    }
    pub fn invalidate(&mut self, path: &str) {
        self.requested.remove(path);
    }
}

/// One thumbnail worker thread: renders requests in order and sends the results on.
fn worker(
    root: std::path::PathBuf,
    requests: Receiver<Request>,
    results: SyncSender<Result<Thumbnail, (String, String)>>,
) {
    std::thread::spawn(move || {
        let mut renderer = None;
        for request in requests {
            let image = match request.source {
                Source::Material(material) => derived(&root, &material, request.shot),
                Source::Bytes(bytes) => master(
                    &root,
                    &request.path,
                    Some(&bytes),
                    request.shot,
                    &mut renderer,
                ),
                Source::File => master(&root, &request.path, None, request.shot, &mut renderer),
            };
            let result = match image {
                Ok(image) => Ok(Thumbnail {
                    key: request.key,
                    image: present(&image, request.pixels),
                }),
                Err(error) => Err((request.key, error.to_string())),
            };
            if results.send(result).is_err() {
                break;
            }
        }
    });
}

/// The square master image for a source, from the content-addressed cache when the
/// source, its import settings and its dependencies are unchanged.
fn master(
    root: &Path,
    path: &str,
    override_source: Option<&[u8]>,
    shot: Shot,
    renderer: &mut Option<Renderer>,
) -> anyhow::Result<RgbaImage> {
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
    // Cache identity includes referenced textures and external glTF buffers.
    if let Ok(value) = ara_json::parse(std::str::from_utf8(&inputs[..source_len]).unwrap_or("")) {
        dependency_bytes(root, source.parent().unwrap_or(root), &value, &mut inputs);
    }
    inputs.extend(format!("ara-thumb-v4-{MASTER}").as_bytes());
    inputs.push(shot.tag());
    let key = ara_asset::Digest::of(&inputs).to_string();
    let cache = root.join(".ara/cache/thumbnails");
    std::fs::create_dir_all(&cache)?;
    let target = cache.join(format!("{key}.png"));
    if target.exists() {
        return Ok(image::open(target)?.to_rgba8());
    }
    let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("");
    let image = match extension {
        "png" | "jpg" | "jpeg" | "hdr" | "exr" => texture(&image::open(&source)?.to_rgba8()),
        "fbx" => studio::shoot(
            studio::Subject::clay(super::cooked_mesh::load(root, path)?),
            renderer,
            &root.join(".ara/cache/thumbnail-gpu"),
        )?,
        "glb" | "gltf" => studio::shoot(
            studio::Subject::model(&*crate::world::gltf::published(&source, PUBLISH_WAIT)?)?,
            renderer,
            &root.join(".ara/cache/thumbnail-gpu"),
        )?,
        "material" | "graph" => {
            let text = std::str::from_utf8(&inputs[..source_len])?;
            // Both views compile the same material AST; cached images are presentation only.
            let material = if extension == "material" {
                ara_material::dsl::parse(text, "gr_preview")
                    .map_err(|d| anyhow::anyhow!("{d:?}"))?
            } else {
                let mut db = ara_schema::SchemaDb::new();
                ara_material::register(&mut db).map_err(|d| anyhow::anyhow!(d.to_string()))?;
                let graph =
                    ara_graph::Graph::read(text, &db).map_err(|d| anyhow::anyhow!("{d:?}"))?;
                if graph.header.domain == "vfx" {
                    let effect = ara_vfx::Effect::from_graph(&graph)
                        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
                    let image = super::vfx_preview::capture(root, &effect)?;
                    image.save(&target)?;
                    return Ok(image);
                }
                ara_material::Material::from_graph(&graph).map_err(|d| anyhow::anyhow!("{d:?}"))?
            };
            super::material_preview::capture(root, &material, shot)?
        }
        _ => anyhow::bail!("no thumbnail decoder for {extension}"),
    };
    image.save(&target)?;
    Ok(image)
}

/// A derived material's preview, cached by its text and shot like file masters.
fn derived(
    root: &Path,
    material: &ara_material::Material,
    shot: Shot,
) -> anyhow::Result<RgbaImage> {
    let text = ara_material::dsl::print(material).map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let mut inputs = text.into_bytes();
    inputs.extend(format!("ara-node-preview-v1-{MASTER}").as_bytes());
    inputs.push(shot.tag());
    let cache = root.join(".ara/cache/thumbnails");
    std::fs::create_dir_all(&cache)?;
    let target = cache.join(format!("{}.png", ara_asset::Digest::of(&inputs)));
    if target.exists() {
        return Ok(image::open(target)?.to_rgba8());
    }
    let image = super::material_preview::capture(root, material, shot)?;
    image.save(&target)?;
    Ok(image)
}

/// A texture fitted into the square on the studio backdrop; transparent texels show
/// a quiet checkerboard. Small sources scale up with nearest so pixel art stays sharp.
fn texture(source: &RgbaImage) -> RgbaImage {
    let (width, height) = source.dimensions();
    let largest = width.max(height).max(1);
    let size = [
        (width * MASTER / largest).max(1),
        (height * MASTER / largest).max(1),
    ];
    let filter = if largest < MASTER {
        image::imageops::FilterType::Nearest
    } else {
        image::imageops::FilterType::Lanczos3
    };
    let fitted = image::imageops::resize(source, size[0], size[1], filter);
    let offset = [(MASTER - size[0]) / 2, (MASTER - size[1]) / 2];
    RgbaImage::from_fn(MASTER, MASTER, |x, y| {
        let backdrop = studio::backdrop(x, y, MASTER);
        let inside =
            x >= offset[0] && y >= offset[1] && x < offset[0] + size[0] && y < offset[1] + size[1];
        if !inside {
            return backdrop;
        }
        let texel = fitted.get_pixel(x - offset[0], y - offset[1]);
        let check = if (x / 32 + y / 32) % 2 == 0 { 92. } else { 66. };
        let alpha = f32::from(texel[3]) / 255.;
        let mix = |c: u8| (f32::from(c) * alpha + check * (1. - alpha)).round() as u8;
        image::Rgba([mix(texel[0]), mix(texel[1]), mix(texel[2]), 255])
    })
}

/// Resample a master to `pixels` square with Lanczos and round its corners
/// (radius 1/16 of the edge, as straight alpha over the backdrop colour).
pub(super) fn present(master: &RgbaImage, pixels: u32) -> RgbaImage {
    let mut image = if master.width() == pixels && master.height() == pixels {
        master.clone()
    } else {
        image::imageops::resize(
            master,
            pixels,
            pixels,
            image::imageops::FilterType::Lanczos3,
        )
    };
    let radius = pixels as f32 / 16.;
    let edge = pixels as f32;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let p = [x as f32 + 0.5, y as f32 + 0.5];
        let corner = [
            p[0].clamp(radius, edge - radius),
            p[1].clamp(radius, edge - radius),
        ];
        let distance = ((p[0] - corner[0]).powi(2) + (p[1] - corner[1]).powi(2)).sqrt();
        let coverage = (radius - distance + 0.5).clamp(0., 1.);
        pixel[3] = (f32::from(pixel[3]) * coverage).round() as u8;
    }
    image
}

fn dependency_bytes(root: &Path, relative: &Path, value: &serde_json::Value, bytes: &mut Vec<u8>) {
    match value {
        serde_json::Value::String(text) => {
            let path = if text.starts_with('@') {
                ara_schema::grammar::parse_ref(text)
                    .ok()
                    .and_then(|r| r.path)
                    .map(|p| root.join(p))
            } else {
                None
            };
            if let Some(path) = path
                .and_then(|p| p.canonicalize().ok())
                .filter(|p| p.starts_with(root.canonicalize().unwrap_or_default()))
                && let Ok(data) = std::fs::read(path)
            {
                bytes.extend(data);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                dependency_bytes(root, relative, value, bytes);
            }
        }
        serde_json::Value::Object(fields) => {
            if let Some(uri) = fields
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.starts_with("data:"))
                && let Ok(path) = relative.join(uri).canonicalize()
                && path.starts_with(root.canonicalize().unwrap_or_default())
                && let Ok(data) = std::fs::read(path)
            {
                bytes.extend(data);
            }
            for value in fields.values() {
                dependency_bytes(root, relative, value, bytes);
            }
        }
        _ => {}
    }
}

/// Stable image resource identity; thumbnail resources use a bounded rotating slot pool.
pub(crate) fn image_name(slot: usize) -> String {
    format!("asset-thumbnail-{slot}")
}

/// The UI image handle of a slot (what `<image src>` resolves `image_name` to).
pub(crate) fn image_id(slot: usize) -> u32 {
    ara_ui::source::stable_id(&image_name(slot)) as u32 & 0x7fff_ffff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_resamples_and_rounds_only_the_corners() {
        let master = RgbaImage::from_pixel(MASTER, MASTER, image::Rgba([90, 90, 90, 255]));
        let tile = present(&master, TILE);
        assert_eq!(tile.dimensions(), (TILE, TILE));
        assert_eq!(tile.get_pixel(0, 0)[3], 0, "corner is cut");
        assert_eq!(tile.get_pixel(TILE / 2, 0)[3], 255, "edges stay opaque");
        assert_eq!(tile.get_pixel(TILE / 2, TILE / 2)[0], 90);
    }

    #[test]
    fn textures_keep_their_aspect_on_the_backdrop() {
        let wide = RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 255]));
        let square = texture(&wide);
        assert_eq!(square.dimensions(), (MASTER, MASTER));
        assert_eq!(square.get_pixel(MASTER / 2, MASTER / 2)[0], 255);
        assert_ne!(square.get_pixel(MASTER / 2, 4)[0], 255, "letterboxed");
    }
}
