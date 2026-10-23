//! UI images: pinned Lucide icons (white, tinted by CSS color), the Ara wordmark,
//! and project/template previews decoded off the UI thread.
use ara_ui::source::stable_id;
use ara_ui_render::gpu::{Image, Renderer};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../../assets/icons/", $name, ".svg")))),*]
    };
}
/// Lucide subset (ISC, `assets/icons/LICENSE`), shared revision with the editor.
const ICONS: &[(&str, &str)] = icons![
    "activity",
    "arrow-up-down",
    "book-open",
    "bot",
    "box",
    "check",
    "chevron-down",
    "chevron-right",
    "circle-alert",
    "circle-check",
    "circle-dot",
    "clock",
    "copy",
    "download",
    "ellipsis",
    "external-link",
    "file-plus",
    "folder",
    "folder-open",
    "folder-plus",
    "gamepad-2",
    "graduation-cap",
    "hard-drive",
    "info",
    "layout-grid",
    "link",
    "list",
    "loader-circle",
    "monitor",
    "moon",
    "package",
    "pencil",
    "pin",
    "play",
    "plug",
    "plus",
    "refresh-cw",
    "search",
    "server",
    "sparkles",
    "square",
    "sun",
    "terminal",
    "trash",
    "triangle-alert",
    "user",
    "users",
    "x",
    "file",
];
const WORDMARK: &str = include_str!("../../../../assets/brand/ara-wordmark.svg");
const WORDMARK_LIGHT: &str = include_str!("../../../../assets/brand/ara-wordmark-light.svg");

/// The texture ID `ara_ui` derives from an image `src`.
pub fn texture(src: &str) -> u32 {
    stable_id(src) as u32 & 0x7fff_ffff
}

fn rasterize(svg: &str, size: [u32; 2]) -> anyhow::Result<Image> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())?;
    let mut pixels = resvg::tiny_skia::Pixmap::new(size[0], size[1])
        .ok_or_else(|| anyhow::anyhow!("image extent"))?;
    let view = tree.size();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            size[0] as f32 / view.width(),
            size[1] as f32 / view.height(),
        ),
        &mut pixels.as_mut(),
    );
    // tiny-skia stores premultiplied pixels; the UI image contract is straight alpha.
    let mut rgba = pixels.take();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] > 0 {
            for n in 0..3 {
                pixel[n] = ((u32::from(pixel[n]) * 255) / u32::from(pixel[3])).min(255) as u8;
            }
        }
    }
    Ok(Image {
        width: size[0],
        height: size[1],
        rgba,
    })
}

/// Rasterize icons and the wordmark for this device scale. Themes never re-rasterize:
/// icons are white and tinted per element; each theme has its own wordmark.
pub fn install(renderer: &mut Renderer, dpi: f32) -> anyhow::Result<()> {
    let size = (16. * dpi.max(2.)).ceil() as u32;
    for (name, svg) in ICONS {
        let image = rasterize(&svg.replace("currentColor", "#ffffff"), [size, size])?;
        renderer
            .image(texture(&format!("icon-{name}")), image)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let scale = dpi.max(1.) * 2.;
    let mark = [(64. * scale).round() as u32, (30. * scale).round() as u32];
    for (src, svg) in [
        ("brand-wordmark", WORDMARK),
        ("brand-wordmark-light", WORDMARK_LIGHT),
    ] {
        renderer
            .image(texture(src), rasterize(svg, mark)?)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    renderer
        .set_raster_scale(dpi)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

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
