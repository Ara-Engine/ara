//! Bounded thumbnail queue for Assets tiles: texture files only. Decoding and cache IO stay
//! off-frame. Each texture renders once as a square master on the studio backdrop and is
//! cached by content; each view gets a Lanczos resample at its pixel size with rounded corners,
//! so the UI's single-mip bilinear sampler never minifies more than 2:1.
//!
//! Models, materials, effects and prefabs have no picture until the renderer can draw them
//! (`Previews::previewable` leaves them out; their tiles show the type's glyph). A texture
//! whose file is still being copied into the project is [`Done::Pending`], not a failure:
//! nothing waits on it and nothing is reported; call [`Thumbnails::retry_pending`] when the
//! import progress moves.
use image::{Rgba, RgbaImage};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::JoinHandle,
};

/// Edge of a master image in pixels; views resample it.
pub(crate) const MASTER: u32 = 512;

/// Pixel size of Assets grid tiles: 2× their 64 px logical thumbnail.
pub(crate) const TILE: u32 = 128;

/// A finished tile picture and the key it was asked for under.
pub(crate) struct Thumbnail {
    pub key: String,
    pub image: RgbaImage,
}

/// What a finished request produced.
pub(crate) enum Done {
    /// The image, ready to upload.
    Image(Thumbnail),
    /// The file has not arrived: show its pending state; it is asked again by
    /// [`Thumbnails::retry_pending`].
    Pending(String),
    /// It cannot decode: the tile keeps its glyph.
    Unreadable,
}

struct Request {
    path: String,
    key: String,
    pixels: u32,
}

/// The queue and its one worker.
pub(crate) struct Thumbnails {
    send: Option<SyncSender<Request>>,
    receive: Option<Receiver<Done>>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    requested: BTreeSet<String>,
    /// Keys whose file had not arrived, asked again by `retry_pending`.
    pending: BTreeSet<String>,
}

impl Thumbnails {
    pub fn new(root: &Path) -> Self {
        let (send, requests) = mpsc::sync_channel::<Request>(48);
        let (results, receive) = mpsc::sync_channel(8);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker = worker(root.to_owned(), requests, results, Arc::clone(&stopping));
        Self {
            send: Some(send),
            receive: Some(receive),
            stopping,
            worker: Some(worker),
            requested: BTreeSet::new(),
            pending: BTreeSet::new(),
        }
    }

    /// An Assets grid tile, keyed by its project path. A full queue drops it: call again to
    /// retry.
    pub fn request(&mut self, path: &str) {
        if self.requested.contains(path) {
            return;
        }
        let request = Request {
            path: path.into(),
            key: path.into(),
            pixels: TILE.clamp(16, MASTER),
        };
        if self
            .send
            .as_ref()
            .is_some_and(|queue| queue.try_send(request).is_ok())
        {
            self.requested.insert(path.into());
        }
    }

    /// Finished requests (up to 8 a frame).
    pub fn poll(&mut self) -> Vec<Done> {
        let done: Vec<Done> = self
            .receive
            .as_ref()
            .into_iter()
            .flat_map(|receive| receive.try_iter())
            .take(8)
            .collect();
        for result in &done {
            if let Done::Pending(key) = result {
                self.pending.insert(key.clone());
            }
        }
        done
    }

    /// Ask again for every file that had not arrived (call when the import progress moves);
    /// true when there were any.
    pub fn retry_pending(&mut self) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        for key in std::mem::take(&mut self.pending) {
            self.requested.remove(&key);
        }
        true
    }

    pub fn invalidate_all(&mut self) {
        self.requested.clear();
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        drop(self.send.take());
        // The worker may be blocked in the bounded results send. Disconnect that receiver
        // before joining, including when the UI never polled its images.
        drop(self.receive.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The worker thread; each result is sent the moment it lands.
fn worker(
    root: std::path::PathBuf,
    requests: Receiver<Request>,
    results: SyncSender<Done>,
    stopping: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(request) = requests.recv() {
            if stopping.load(Ordering::Acquire) {
                return;
            }
            if results.send(finish(&root, request)).is_err() {
                return;
            }
        }
    })
}

/// Renders one request into its result.
fn finish(root: &Path, request: Request) -> Done {
    match master(root, &request.path) {
        Ok(Some(image)) => Done::Image(Thumbnail {
            key: request.key,
            image: present(&image, request.pixels),
        }),
        Ok(None) => Done::Pending(request.key),
        // A file that will not decode keeps its glyph; nothing is reported for a tile.
        Err(_) => Done::Unreadable,
    }
}

/// The square master image of a texture file, from the content-addressed cache when the file and
/// its import settings are unchanged. Paths are authored-root-relative. `None` while the file
/// has not arrived.
fn master(root: &Path, path: &str) -> anyhow::Result<Option<RgbaImage>> {
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    let source = match authored.join(path).canonicalize() {
        Ok(source) => source,
        // An import still copying its file into the project: pending.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        source.starts_with(authored.canonicalize()?),
        "thumbnail source escapes project"
    );
    let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("");
    anyhow::ensure!(
        crate::names::is_texture(extension) && extension != "ktx2",
        "no thumbnail decoder for {extension}"
    );
    let mut inputs = std::fs::read(&source)?;
    if let Ok(settings) = std::fs::read(authored.join(format!("{path}.import.json"))) {
        inputs.extend(settings);
    }
    inputs.extend(format!("ara-thumb-v4-{MASTER}").as_bytes());
    let key = ara_asset::Digest::of(&inputs).to_string();
    let cache = root.join(".ara/cache/thumbnails");
    std::fs::create_dir_all(&cache)?;
    let target = cache.join(format!("{key}.png"));
    if target.exists() {
        return Ok(Some(image::open(target)?.to_rgba8()));
    }
    let image = texture(&image::open(&source)?.to_rgba8());
    image.save(&target)?;
    Ok(Some(image))
}

/// The shared neutral backdrop: a cool grey gradient with a soft light behind the subject.
fn backdrop(x: u32, y: u32, size: u32) -> Rgba<u8> {
    let u = (x as f32 + 0.5) / size as f32;
    let v = (y as f32 + 0.5) / size as f32;
    let glow = (1. - ((u - 0.5).powi(2) + (v - 0.42).powi(2)).sqrt() * 1.6).clamp(0., 1.);
    let shade = |top: f32, bottom: f32| {
        let value = top + (bottom - top) * v + glow * glow * 10.;
        value.round().clamp(0., 255.) as u8
    };
    Rgba([shade(60., 34.), shade(63., 36.), shade(69., 40.), 255])
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
        let backdrop = backdrop(x, y, MASTER);
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
fn present(master: &RgbaImage, pixels: u32) -> RgbaImage {
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

#[cfg(test)]
#[path = "thumbnails/shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_thumbnail_reads_authored_pixels_and_keeps_the_cache_outside_content() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-editor/thumbnail-paths")
            .join(std::process::id().to_string());
        let source = root.join(ara_doc::AUTHORING_ROOT).join("textures/chip.png");
        std::fs::create_dir_all(source.parent().expect("texture folder")).expect("scratch");
        let color = image::Rgba([17, 124, 200, 255]);
        RgbaImage::from_pixel(1, 1, color)
            .save(&source)
            .expect("source pixels");
        let thumbnail = master(&root, "textures/chip.png")
            .expect("authored texture decodes")
            .expect("texture is ready");
        assert_eq!(*thumbnail.get_pixel(MASTER / 2, MASTER / 2), color);
        assert!(root.join(".ara/cache/thumbnails").is_dir());
        assert!(!root.join(ara_doc::AUTHORING_ROOT).join(".ara").exists());
    }

    #[test]
    fn a_file_not_copied_in_yet_is_pending_and_a_model_has_no_decoder() {
        let root = std::env::temp_dir().join(format!("ara-thumb-pending-{}", std::process::id()));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("models"))
            .expect("scratch project");
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("models/box.gltf"),
            r#"{"asset":{"version":"2.0"}}"#,
        )
        .expect("model");
        // A texture still being copied in is pending, not a failure.
        assert!(
            master(&root, "textures/new.png")
                .expect("a file not copied yet is not an error")
                .is_none()
        );
        // Models have no picture until the renderer can draw them.
        assert!(master(&root, "models/box.gltf").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

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
