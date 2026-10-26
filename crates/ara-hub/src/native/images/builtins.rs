//! Lazy built-in UI assets; every page keeps its images after first use.
use ara_ui::source::stable_id;
use ara_ui_render::gpu::{Image, Renderer};
use std::collections::{BTreeMap, BTreeSet};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../../../assets/icons/", $name, ".svg")))),*]
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
const WORDMARK: &str = include_str!("../../../../../assets/brand/ara-wordmark.svg");
const WORDMARK_LIGHT: &str = include_str!("../../../../../assets/brand/ara-wordmark-light.svg");

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

/// Static image sources are cheap; decoding and GPU uploads happen only on first use.
enum Source {
    Icon(&'static str),
    Wordmark(&'static str),
    Starter,
}

pub(crate) struct Builtins {
    sources: BTreeMap<u32, Source>,
    loaded: BTreeSet<u32>,
    dpi: f32,
}
impl Builtins {
    pub(crate) fn new(dpi: f32) -> Self {
        let mut sources: BTreeMap<_, _> = ICONS
            .iter()
            .map(|(name, svg)| (texture(&format!("icon-{name}")), Source::Icon(svg)))
            .collect();
        sources.insert(texture("brand-wordmark"), Source::Wordmark(WORDMARK));
        sources.insert(
            texture("brand-wordmark-light"),
            Source::Wordmark(WORDMARK_LIGHT),
        );
        sources.insert(texture("template-starter"), Source::Starter);
        Self {
            sources,
            loaded: BTreeSet::new(),
            dpi,
        }
    }
    pub(crate) fn loaded(&self) -> usize {
        self.loaded.len()
    }
    /// Prepare every built-in referenced by this frame, retaining earlier pages/themes.
    pub(crate) fn prepare(
        &mut self,
        renderer: &mut Renderer,
        display: &ara_ui::paint::DisplayList,
    ) -> anyhow::Result<()> {
        renderer
            .set_raster_scale(self.dpi)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for command in &display.commands {
            let ara_ui::paint::Draw::Image { image: id, .. } = command else {
                continue;
            };
            if self.loaded.contains(id) {
                continue;
            }
            let Some(source) = self.sources.get(id) else {
                continue;
            };
            let image = match source {
                Source::Icon(svg) => {
                    let size = (16. * self.dpi.max(2.)).ceil() as u32;
                    rasterize(&svg.replace("currentColor", "#ffffff"), [size, size])?
                }
                Source::Wordmark(svg) => {
                    let scale = self.dpi.max(1.) * 2.;
                    rasterize(
                        svg,
                        [(64. * scale).round() as u32, (30. * scale).round() as u32],
                    )?
                }
                Source::Starter => {
                    let image = image::load_from_memory(include_bytes!(
                        "../../../../../release/starter.png"
                    ))?
                    .into_rgba8();
                    Image {
                        width: image.width(),
                        height: image.height(),
                        rgba: image.into_raw(),
                    }
                }
            };
            renderer
                .image(*id, image)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            self.loaded.insert(*id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Hub, Intent, Prefs, View},
        native::App,
    };

    #[test]
    fn first_frame_prepares_only_visible_assets_and_later_pages_keep_their_images() {
        let mut hub = Hub::new(Prefs::default(), "target/hub-assets-tests".into(), None);
        let mut app = App::new([1440., 900.], Vec::new()).expect("app");
        let mut assets = Builtins::new(1.);
        let mut renderer = Renderer::default();
        app.rebuild(&mut hub, crate::capture::NOW)
            .expect("first view");
        assets
            .prepare(&mut renderer, &app.ui.display)
            .expect("first images");
        assert!(assets.loaded.contains(&texture("brand-wordmark")));
        assert!(!assets.loaded.contains(&texture("brand-wordmark-light")));
        assert!(!assets.loaded.contains(&texture("template-starter")));
        assert!(
            assets.loaded.len() < assets.sources.len() / 2,
            "first frame must not upload every page's images"
        );
        let first = assets.loaded();
        assets
            .prepare(&mut renderer, &app.ui.display)
            .expect("unchanged frame");
        assert_eq!(assets.loaded(), first, "unchanged images are retained");
        crate::capture::seed(&mut hub);
        for light in [false, true] {
            hub.prefs.light = light;
            for view in View::ALL {
                hub.intent(Intent::Navigate(view));
                app.rebuild(&mut hub, crate::capture::NOW).expect("page");
                assets
                    .prepare(&mut renderer, &app.ui.display)
                    .expect("page images");
                renderer
                    .prepare(
                        &app.ui.display,
                        &app.text.assets,
                        [1440, 900],
                        ara_ui_render::atlas::TextMode::Raster,
                    )
                    .expect("all visible images resolve on the first visit");
            }
        }
        assert!(assets.loaded.contains(&texture("template-starter")));
        assert!(assets.loaded.contains(&texture("brand-wordmark-light")));
    }
}
