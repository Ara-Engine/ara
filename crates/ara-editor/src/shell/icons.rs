//! Pinned Lucide assets, rasterized once per theme/device scale, never per frame.
use ara_ui::source::stable_id;
use ara_ui_render::gpu::{Image, Renderer};

pub(super) fn icon(id: &str, name: &str) -> String {
    format!("<image id='{id}-icon' class='icon' src='icon-{name}' />")
}
pub(crate) fn install(renderer: &mut Renderer, light: bool, dpi: f32) -> ara_ui::Result<()> {
    let tokens = if light {
        ara_ui::theme::EditorTheme::Light
    } else {
        ara_ui::theme::EditorTheme::Dark
    }
    .tokens()?;
    let color = tokens.get("--muted-foreground").ok_or("icon theme token")?;
    let size = (16. * dpi.max(2.)).ceil() as u32;
    for (name, svg) in ASSETS {
        let svg = svg.replace("currentColor", color);
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())?;
        let mut pixels = resvg::tiny_skia::Pixmap::new(size, size).ok_or("icon extent")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(size as f32 / 24., size as f32 / 24.),
            &mut pixels.as_mut(),
        );
        // tiny-skia stores premultiplied pixels; the UI image contract is straight alpha.
        let mut rgba = pixels.take();
        for pixel in rgba.chunks_exact_mut(4) {
            if pixel[3] > 0 {
                for n in 0..3 {
                    pixel[n] = ((u32::from(pixel[n]) * 255) / u32::from(pixel[3])).min(255) as u8;
                }
            }
        }
        renderer.image(
            stable_id(&format!("icon-{name}")) as u32 & 0x7fff_ffff,
            Image {
                width: size,
                height: size,
                rgba,
            },
        )?;
    }
    renderer.set_raster_scale(dpi)?;
    Ok(())
}
const ASSETS: &[(&str, &str)] = &[
    ("activity", include_str!("../../assets/icons/activity.svg")),
    (
        "arrow-right",
        include_str!("../../assets/icons/arrow-right.svg"),
    ),
    ("bot", include_str!("../../assets/icons/bot.svg")),
    ("box", include_str!("../../assets/icons/box.svg")),
    ("camera", include_str!("../../assets/icons/camera.svg")),
    ("check", include_str!("../../assets/icons/check.svg")),
    (
        "chevron-down",
        include_str!("../../assets/icons/chevron-down.svg"),
    ),
    (
        "chevron-right",
        include_str!("../../assets/icons/chevron-right.svg"),
    ),
    ("circle", include_str!("../../assets/icons/circle.svg")),
    (
        "circle-check",
        include_str!("../../assets/icons/circle-check.svg"),
    ),
    (
        "circle-dot",
        include_str!("../../assets/icons/circle-dot.svg"),
    ),
    ("command", include_str!("../../assets/icons/command.svg")),
    (
        "crosshair",
        include_str!("../../assets/icons/crosshair.svg"),
    ),
    ("ellipsis", include_str!("../../assets/icons/ellipsis.svg")),
    ("eye", include_str!("../../assets/icons/eye.svg")),
    ("eye-off", include_str!("../../assets/icons/eye-off.svg")),
    ("file", include_str!("../../assets/icons/file.svg")),
    (
        "file-code",
        include_str!("../../assets/icons/file-code.svg"),
    ),
    (
        "file-image",
        include_str!("../../assets/icons/file-image.svg"),
    ),
    ("focus", include_str!("../../assets/icons/focus.svg")),
    ("folder", include_str!("../../assets/icons/folder.svg")),
    ("gauge", include_str!("../../assets/icons/gauge.svg")),
    ("globe", include_str!("../../assets/icons/globe.svg")),
    ("grid-2x2", include_str!("../../assets/icons/grid-2x2.svg")),
    ("layers", include_str!("../../assets/icons/layers.svg")),
    (
        "lightbulb",
        include_str!("../../assets/icons/lightbulb.svg"),
    ),
    ("lock", include_str!("../../assets/icons/lock.svg")),
    (
        "lock-open",
        include_str!("../../assets/icons/lock-open.svg"),
    ),
    ("magnet", include_str!("../../assets/icons/magnet.svg")),
    ("moon", include_str!("../../assets/icons/moon.svg")),
    ("move-3d", include_str!("../../assets/icons/move-3d.svg")),
    ("package", include_str!("../../assets/icons/package.svg")),
    ("pause", include_str!("../../assets/icons/pause.svg")),
    ("play", include_str!("../../assets/icons/play.svg")),
    ("plus", include_str!("../../assets/icons/plus.svg")),
    (
        "rotate-3d",
        include_str!("../../assets/icons/rotate-3d.svg"),
    ),
    ("scaling", include_str!("../../assets/icons/scaling.svg")),
    ("search", include_str!("../../assets/icons/search.svg")),
    (
        "settings-2",
        include_str!("../../assets/icons/settings-2.svg"),
    ),
    (
        "step-forward",
        include_str!("../../assets/icons/step-forward.svg"),
    ),
    ("sun", include_str!("../../assets/icons/sun.svg")),
    ("undo-2", include_str!("../../assets/icons/undo-2.svg")),
    ("x", include_str!("../../assets/icons/x.svg")),
];
