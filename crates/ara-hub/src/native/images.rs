//! UI images: pinned Lucide icons (white, tinted by CSS color), the Ara wordmark,
//! and project/template previews decoded off the UI thread.
use ara_ui::source::stable_id;
use ara_ui_render::gpu::{Image, Renderer};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

mod builtins;
pub(super) use builtins::Builtins;
pub use builtins::texture;

/// The largest preview kept per image: 16:9, enough for a 2× card.
const PREVIEW: [u32; 2] = [800, 450];

/// Decode a preview: center-crop to 16:9 and downscale.
fn decode(path: &Path) -> anyhow::Result<Image> {
    let image = image::open(path)?.to_rgba8();
    let (w, h) = image.dimensions();
    let (cw, ch) = if w * 9 > h * 16 {
        (h * 16 / 9, h)
    } else {
        (w, w * 9 / 16)
    };
    let cropped = image::imageops::crop_imm(&image, (w - cw) / 2, (h - ch) / 2, cw, ch).to_image();
    let scaled = if cw > PREVIEW[0] {
        image::imageops::resize(
            &cropped,
            PREVIEW[0],
            PREVIEW[1],
            image::imageops::FilterType::Triangle,
        )
    } else {
        cropped
    };
    Ok(Image {
        width: scaled.width(),
        height: scaled.height(),
        rgba: scaled.into_raw(),
    })
}

/// Preview images by path: requested by the views, decoded on worker threads,
/// uploaded on the UI thread. Failures are remembered so the fallback tile stays.
pub struct Thumbnails {
    loaded: BTreeMap<PathBuf, String>,
    asked: BTreeSet<PathBuf>,
    failed: usize,
    send: Sender<(PathBuf, anyhow::Result<Image>)>,
    done: Receiver<(PathBuf, anyhow::Result<Image>)>,
}
impl Default for Thumbnails {
    fn default() -> Self {
        let (send, done) = mpsc::channel();
        Self {
            loaded: BTreeMap::new(),
            asked: BTreeSet::new(),
            failed: 0,
            send,
            done,
        }
    }
}
impl Thumbnails {
    /// The `src` of a loaded preview.
    pub fn get(&self, path: &Path) -> Option<String> {
        self.loaded.get(path).cloned()
    }
    /// True while requested previews are still decoding.
    pub fn pending(&self) -> bool {
        self.asked.len() > self.loaded.len() + self.failed
    }
    /// Start decoding previews not seen before.
    pub fn request(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if self.asked.insert(path.clone()) {
                let send = self.send.clone();
                let path = path.clone();
                std::thread::spawn(move || {
                    let image = decode(&path);
                    let _ = send.send((path, image));
                    super::wake::wake();
                });
            }
        }
    }
    /// Upload finished previews; true when any became available.
    pub fn upload(&mut self, renderer: &mut Renderer) -> bool {
        let mut changed = false;
        for (path, image) in self.done.try_iter() {
            let src = format!("thumb-{:x}", stable_id(&path.to_string_lossy()));
            match image.map(|image| renderer.image(texture(&src), image)) {
                Ok(Ok(())) => {
                    self.loaded.insert(path, src);
                    changed = true;
                }
                _ => self.failed += 1,
            }
        }
        changed
    }
}
