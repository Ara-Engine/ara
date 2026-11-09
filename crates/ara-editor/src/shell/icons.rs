//! Pinned Lucide assets and simple workspace glyphs, rasterized once per device scale; `tint` images
//! take their element's text color, so themes never re-rasterize icons.
use ara_ui::source::stable_id;
use ara_ui_render::gpu::{Image, Renderer};

/// An icon tinted by the element's CSS `color` (muted by default via `.icon`).
pub(super) fn icon(id: &str, name: &str) -> String {
    format!("<image id='{id}-icon' class='icon' src='icon-{name}' tint='true' />")
}
/// An icon with an extra color class (`accent`, `success`, …).
pub(super) fn icon_in(id: &str, name: &str, class: &str) -> String {
    format!("<image id='{id}-icon' class='icon {class}' src='icon-{name}' tint='true' />")
}
/// Whether a Lucide icon is bundled.
pub(super) fn exists(name: &str) -> bool {
    ASSETS.iter().any(|(n, _)| *n == name)
}
/// Rasterize every icon (white, tinted at draw time), the agent client marks
/// (`icon-agent-<id>`; Claude keeps its brand colour) and the brand logo for `dpi`.
pub(crate) fn install(renderer: &mut Renderer, dpi: f32) -> ara_ui::Result<()> {
    let size = (16. * dpi.max(2.)).ceil() as u32;
    let agents =
        ara_agents::Client::ALL.map(|client| (format!("agent-{}", client.id()), client.logo()));
    for (name, svg) in ASSETS
        .iter()
        .map(|(name, svg)| (name.to_string(), *svg))
        .chain(agents)
    {
        let svg = svg.replace("currentColor", "#ffffff");
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())?;
        let mut pixels = resvg::tiny_skia::Pixmap::new(size, size).ok_or("icon extent")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(size as f32 / 24., size as f32 / 24.),
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
        renderer.image(
            stable_id(&format!("icon-{name}")) as u32 & 0x7fff_ffff,
            Image {
                width: size,
                height: size,
                rgba,
            },
        )?;
    }
    // The menu bar's app logo keeps its own colours (not a tinted icon).
    let logo = (64. * dpi.max(1.)).ceil() as u32; // menu bar (22 px) and About (56 px)
    // The chrome shows the mark alone: drop the app icon's dark tile and its rim.
    let mark = BRAND
        .lines()
        .filter(|line| !line.trim_start().starts_with("<rect"))
        .collect::<Vec<_>>()
        .join("\n");
    let tree = resvg::usvg::Tree::from_str(&mark, &resvg::usvg::Options::default())?;
    let mut pixels = resvg::tiny_skia::Pixmap::new(logo, logo).ok_or("logo extent")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(logo as f32 / 64., logo as f32 / 64.),
        &mut pixels.as_mut(),
    );
    let mut rgba = pixels.take();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] > 0 {
            for n in 0..3 {
                pixel[n] = ((u32::from(pixel[n]) * 255) / u32::from(pixel[3])).min(255) as u8;
            }
        }
    }
    renderer.image(
        stable_id("brand-logo") as u32 & 0x7fff_ffff,
        Image {
            width: logo,
            height: logo,
            rgba,
        },
    )?;
    super::overlay::gizmos::install(renderer, dpi)?;
    renderer.set_raster_scale(dpi)?;
    Ok(())
}
/// A bundled Lucide icon's SVG source (Scene gizmo glyphs rasterize their own size).
pub(crate) fn svg(name: &str) -> Option<&'static str> {
    ASSETS.iter().find(|(n, _)| *n == name).map(|(_, svg)| *svg)
}
/// The engine's small-size icon, authored for compact native chrome (64-unit view box).
const BRAND: &str = include_str!("../../../../assets/brand/icons/engine/ara-engine-small.svg");
const ASSETS: &[(&str, &str)] = &[
    // Scene gizmo glyphs.
    (
        "cloud-fog",
        include_str!("../../assets/icons/cloud-fog.svg"),
    ),
    (
        "audio-lines",
        include_str!("../../assets/icons/audio-lines.svg"),
    ),
    // View modes.
    ("sun-dim", include_str!("../../assets/icons/sun-dim.svg")),
    ("hash", include_str!("../../assets/icons/hash.svg")),
    ("layers-2", include_str!("../../assets/icons/layers-2.svg")),
    ("flame", include_str!("../../assets/icons/flame.svg")),
    (
        "arrow-up-from-dot",
        include_str!("../../assets/icons/arrow-up-from-dot.svg"),
    ),
    (
        "wireframe",
        include_str!("../../assets/icons/wireframe.svg"),
    ),
    (
        "lit-wireframe",
        include_str!("../../assets/icons/lit-wireframe.svg"),
    ),
    // Windows 11 caption glyphs: 10 px, 1 px strokes at 16 logical px.
    (
        "caption-minimize",
        include_str!("../../assets/icons/caption-minimize.svg"),
    ),
    (
        "caption-maximize",
        include_str!("../../assets/icons/caption-maximize.svg"),
    ),
    (
        "caption-restore",
        include_str!("../../assets/icons/caption-restore.svg"),
    ),
    (
        "caption-close",
        include_str!("../../assets/icons/caption-close.svg"),
    ),
    ("star", include_str!("../../assets/icons/star.svg")),
    ("bookmark", include_str!("../../assets/icons/bookmark.svg")),
    ("activity", include_str!("../../assets/icons/activity.svg")),
    (
        "arrow-right",
        include_str!("../../assets/icons/arrow-right.svg"),
    ),
    (
        "arrow-up-down",
        include_str!("../../assets/icons/arrow-up-down.svg"),
    ),
    (
        "book-open",
        include_str!("../../assets/icons/book-open.svg"),
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
        "chevron-left",
        include_str!("../../assets/icons/chevron-left.svg"),
    ),
    (
        "chevron-right",
        include_str!("../../assets/icons/chevron-right.svg"),
    ),
    (
        "chevron-up",
        include_str!("../../assets/icons/chevron-up.svg"),
    ),
    (
        "circle-alert",
        include_str!("../../assets/icons/circle-alert.svg"),
    ),
    (
        "circle-check",
        include_str!("../../assets/icons/circle-check.svg"),
    ),
    (
        "circle-dot",
        include_str!("../../assets/icons/circle-dot.svg"),
    ),
    (
        "circle-play",
        include_str!("../../assets/icons/circle-play.svg"),
    ),
    ("circle-x", include_str!("../../assets/icons/circle-x.svg")),
    ("circle", include_str!("../../assets/icons/circle.svg")),
    (
        "clapperboard",
        include_str!("../../assets/icons/clapperboard.svg"),
    ),
    (
        "clipboard",
        include_str!("../../assets/icons/clipboard.svg"),
    ),
    ("clock", include_str!("../../assets/icons/clock.svg")),
    ("command", include_str!("../../assets/icons/command.svg")),
    ("contrast", include_str!("../../assets/icons/contrast.svg")),
    ("copy", include_str!("../../assets/icons/copy.svg")),
    (
        "crosshair",
        include_str!("../../assets/icons/crosshair.svg"),
    ),
    ("ear", include_str!("../../assets/icons/ear.svg")),
    (
        "ellipsis-vertical",
        include_str!("../../assets/icons/ellipsis-vertical.svg"),
    ),
    ("ellipsis", include_str!("../../assets/icons/ellipsis.svg")),
    (
        "external-link",
        include_str!("../../assets/icons/external-link.svg"),
    ),
    ("eye-off", include_str!("../../assets/icons/eye-off.svg")),
    ("eye", include_str!("../../assets/icons/eye.svg")),
    (
        "file-code",
        include_str!("../../assets/icons/file-code.svg"),
    ),
    (
        "file-image",
        include_str!("../../assets/icons/file-image.svg"),
    ),
    (
        "file-plus",
        include_str!("../../assets/icons/file-plus.svg"),
    ),
    ("file", include_str!("../../assets/icons/file.svg")),
    ("flag", include_str!("../../assets/icons/flag.svg")),
    (
        "flashlight",
        include_str!("../../assets/icons/flashlight.svg"),
    ),
    ("focus", include_str!("../../assets/icons/focus.svg")),
    (
        "folder-open",
        include_str!("../../assets/icons/folder-open.svg"),
    ),
    ("folder", include_str!("../../assets/icons/folder.svg")),
    ("funnel", include_str!("../../assets/icons/funnel.svg")),
    (
        "gamepad-2",
        include_str!("../../assets/icons/gamepad-2.svg"),
    ),
    ("gauge", include_str!("../../assets/icons/gauge.svg")),
    (
        "git-compare",
        include_str!("../../assets/icons/git-compare.svg"),
    ),
    ("globe", include_str!("../../assets/icons/globe.svg")),
    ("grid-2x2", include_str!("../../assets/icons/grid-2x2.svg")),
    (
        "grip-vertical",
        include_str!("../../assets/icons/grip-vertical.svg"),
    ),
    ("hand", include_str!("../../assets/icons/hand.svg")),
    ("image", include_str!("../../assets/icons/image.svg")),
    ("inbox", include_str!("../../assets/icons/inbox.svg")),
    ("info", include_str!("../../assets/icons/info.svg")),
    ("keyboard", include_str!("../../assets/icons/keyboard.svg")),
    ("layers", include_str!("../../assets/icons/layers.svg")),
    (
        "layout-grid",
        include_str!("../../assets/icons/layout-grid.svg"),
    ),
    (
        "lightbulb",
        include_str!("../../assets/icons/lightbulb.svg"),
    ),
    ("link", include_str!("../../assets/icons/link.svg")),
    (
        "list-tree",
        include_str!("../../assets/icons/list-tree.svg"),
    ),
    (
        "lock-open",
        include_str!("../../assets/icons/lock-open.svg"),
    ),
    ("lock", include_str!("../../assets/icons/lock.svg")),
    (
        "loader-circle",
        include_str!("../../assets/icons/loader-circle.svg"),
    ),
    ("magnet", include_str!("../../assets/icons/magnet.svg")),
    ("map", include_str!("../../assets/icons/map.svg")),
    ("maximize", include_str!("../../assets/icons/maximize.svg")),
    ("menu", include_str!("../../assets/icons/menu.svg")),
    ("minimize", include_str!("../../assets/icons/minimize.svg")),
    ("minus", include_str!("../../assets/icons/minus.svg")),
    ("monitor", include_str!("../../assets/icons/monitor.svg")),
    ("moon", include_str!("../../assets/icons/moon.svg")),
    (
        "mouse-pointer",
        include_str!("../../assets/icons/mouse-pointer.svg"),
    ),
    ("move-3d", include_str!("../../assets/icons/move-3d.svg")),
    (
        "transform",
        include_str!("../../assets/icons/transform.svg"),
    ),
    (
        "package-open",
        include_str!("../../assets/icons/package-open.svg"),
    ),
    ("package", include_str!("../../assets/icons/package.svg")),
    ("palette", include_str!("../../assets/icons/palette.svg")),
    (
        "panel-bottom",
        include_str!("../../assets/icons/panel-bottom.svg"),
    ),
    (
        "panel-left",
        include_str!("../../assets/icons/panel-left.svg"),
    ),
    (
        "panel-right",
        include_str!("../../assets/icons/panel-right.svg"),
    ),
    ("pause", include_str!("../../assets/icons/pause.svg")),
    ("pencil", include_str!("../../assets/icons/pencil.svg")),
    (
        "person-standing",
        include_str!("../../assets/icons/person-standing.svg"),
    ),
    ("pill", include_str!("../../assets/icons/pill.svg")),
    ("pin", include_str!("../../assets/icons/pin.svg")),
    ("pipette", include_str!("../../assets/icons/pipette.svg")),
    ("play", include_str!("../../assets/icons/play.svg")),
    ("plug", include_str!("../../assets/icons/plug.svg")),
    ("plus", include_str!("../../assets/icons/plus.svg")),
    ("redo-2", include_str!("../../assets/icons/redo-2.svg")),
    (
        "refresh-cw",
        include_str!("../../assets/icons/refresh-cw.svg"),
    ),
    (
        "rotate-3d",
        include_str!("../../assets/icons/rotate-3d.svg"),
    ),
    (
        "rotate-ccw",
        include_str!("../../assets/icons/rotate-ccw.svg"),
    ),
    ("save", include_str!("../../assets/icons/save.svg")),
    ("scaling", include_str!("../../assets/icons/scaling.svg")),
    ("scan-eye", include_str!("../../assets/icons/scan-eye.svg")),
    ("scissors", include_str!("../../assets/icons/scissors.svg")),
    ("search", include_str!("../../assets/icons/search.svg")),
    ("server", include_str!("../../assets/icons/server.svg")),
    (
        "settings-2",
        include_str!("../../assets/icons/settings-2.svg"),
    ),
    (
        "sliders-horizontal",
        include_str!("../../assets/icons/sliders-horizontal.svg"),
    ),
    ("sparkles", include_str!("../../assets/icons/sparkles.svg")),
    ("square", include_str!("../../assets/icons/square.svg")),
    (
        "step-forward",
        include_str!("../../assets/icons/step-forward.svg"),
    ),
    (
        "sun-medium",
        include_str!("../../assets/icons/sun-medium.svg"),
    ),
    ("sun", include_str!("../../assets/icons/sun.svg")),
    ("terminal", include_str!("../../assets/icons/terminal.svg")),
    ("trash", include_str!("../../assets/icons/trash.svg")),
    (
        "triangle-alert",
        include_str!("../../assets/icons/triangle-alert.svg"),
    ),
    ("undo-2", include_str!("../../assets/icons/undo-2.svg")),
    ("user", include_str!("../../assets/icons/user.svg")),
    ("users", include_str!("../../assets/icons/users.svg")),
    ("video", include_str!("../../assets/icons/video.svg")),
    ("volume-2", include_str!("../../assets/icons/volume-2.svg")),
    ("wifi", include_str!("../../assets/icons/wifi.svg")),
    ("x", include_str!("../../assets/icons/x.svg")),
];
