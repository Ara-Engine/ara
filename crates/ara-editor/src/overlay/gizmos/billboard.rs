//! Billboard icons: a filled disc with a crisp outline and a bold glyph, drawn far to
//! near. Lights fill with their own colour (dark glyph); other types sit on the popover
//! surface with a glyph in their type colour. 3D icons shrink with distance within
//! limits, and every icon fades out near the camera so it never blocks the view.
//! Glyphs rasterize at their drawn size per device scale ([`install`]).
use super::parts::{Icon, Tint};
use super::shapes::Palette;
use super::wire::Pen;
use super::*;

/// Disc radius at 100 % icon size, in logical pixels.
const RADIUS: f32 = 11.;
/// Glyphs the icons use, with their bundled Lucide SVGs; each rasterizes as `gizmo-<name>`.
const GLYPHS: &[(&str, &str)] = &[
    ("lightbulb", include_str!("../../../assets/icons/lightbulb.svg")),
    ("flashlight", include_str!("../../../assets/icons/flashlight.svg")),
    ("sun", include_str!("../../../assets/icons/sun.svg")),
    ("square", include_str!("../../../assets/icons/square.svg")),
    ("circle", include_str!("../../../assets/icons/circle.svg")),
    ("video", include_str!("../../../assets/icons/video.svg")),
    ("volume-2", include_str!("../../../assets/icons/volume-2.svg")),
    ("audio-lines", include_str!("../../../assets/icons/audio-lines.svg")),
    ("ear", include_str!("../../../assets/icons/ear.svg")),
    ("circle-dot", include_str!("../../../assets/icons/circle-dot.svg")),
    ("layers", include_str!("../../../assets/icons/layers.svg")),
    ("cloud-fog", include_str!("../../../assets/icons/cloud-fog.svg")),
    ("sun-dim", include_str!("../../../assets/icons/sun-dim.svg")),
    ("globe", include_str!("../../../assets/icons/globe.svg")),
    ("sparkles", include_str!("../../../assets/icons/sparkles.svg")),
];

/// Rasterize the gizmo glyphs (white, bolder strokes) for `dpi`, sized for the
/// largest default icon so they never magnify.
pub(crate) fn install(renderer: &mut ara_ui_render::gpu::Renderer, dpi: f32) -> ara_ui::Result<()> {
    let size = (36. * dpi.max(1.)).ceil() as u32;
    for (name, svg) in GLYPHS {
        let svg = svg
            .replace("currentColor", "#ffffff")
            .replace("stroke-width=\"2\"", "stroke-width=\"2.4\"");
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())?;
        let mut pixels = resvg::tiny_skia::Pixmap::new(size, size).ok_or("gizmo glyph extent")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(size as f32 / 24., size as f32 / 24.),
            &mut pixels.as_mut(),
        );
        // Straight alpha: every glyph is white, so colour channels are simply 255.
        let mut rgba = pixels.take();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[..3].fill(255);
        }
        renderer.image(
            glyph(name),
            ara_ui_render::gpu::Image {
                width: size,
                height: size,
                rgba,
            },
        )?;
    }
    Ok(())
}
/// Resource handle of a gizmo glyph image.
fn glyph(name: &str) -> u32 {
    ara_ui::source::stable_id(&format!("gizmo-{name}")) as u32 & 0x7fff_ffff
}

/// One icon to draw.
pub(super) struct Item<'a> {
    pub entity: &'a str,
    pub icon: &'a Icon,
    pub position: Vec3,
    pub detail: Detail,
}

/// Icon radius in logical pixels at `distance` metres (zero: orthographic).
pub(super) fn radius(settings: &Settings, distance: f32) -> f32 {
    let depth = if settings.icons_3d && distance > 0. {
        (12. / distance).sqrt().clamp(0.55, 1.2)
    } else {
        1.
    };
    RADIUS * settings.icon_scale * depth
}
/// Opacity near the camera: icons fade out inside the last 1.5 m.
pub(super) fn near_fade(distance: f32) -> f32 {
    if distance <= 0. {
        return 1.;
    }
    let t = ((distance - 0.4) / 1.5).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Append icons far to near and record each drawn disc in `drawn`.
pub(super) fn draw<'a>(
    overlays: &mut Vec<Overlay>,
    drawn: &mut Vec<(String, [f32; 2], f32)>,
    pen: &Pen,
    settings: &Settings,
    palette: &Palette,
    items: impl Iterator<Item = Item<'a>>,
) {
    let rect = pen.rect();
    let mut shown: Vec<(f32, Item<'a>, [f32; 2])> = items
        .filter_map(|item| {
            let centre = pen.project(item.position)?;
            rect.contains(centre[0], centre[1])
                .then(|| (pen.distance(item.position), item, centre))
        })
        .collect();
    shown.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (distance, item, centre) in shown {
        let fade = near_fade(distance);
        if fade <= 0.01 {
            continue;
        }
        let lit = item.detail != Detail::Subtle;
        let r = radius(settings, distance) * if lit { 1.1 } else { 1. };
        let disc = |overlays: &mut Vec<Overlay>, radius: f32, c: [f32; 4]| {
            overlays.push(Overlay::Line {
                from: centre,
                to: centre,
                width: radius,
                color: [c[0], c[1], c[2], c[3] * fade],
            });
        };
        let tint = palette.tint(item.icon.tint);
        let light = matches!(item.icon.tint, Tint::Light(_));
        // Soft drop shadow, outline ring, fill.
        disc(overlays, r + 2.5, [0., 0., 0., 0.22]);
        let ring = match item.detail {
            Detail::Full => palette.accent,
            Detail::Hover => palette.hover,
            Detail::Subtle if light => [0.05, 0.05, 0.06, 0.85],
            Detail::Subtle => [tint[0], tint[1], tint[2], 0.7],
        };
        disc(overlays, r + if lit { 2. } else { 1.25 }, ring);
        let fill = if light {
            [tint[0], tint[1], tint[2], 0.96]
        } else {
            [
                palette.surface[0],
                palette.surface[1],
                palette.surface[2],
                0.94,
            ]
        };
        disc(overlays, r, fill);
        let ink = if light {
            [0.07, 0.07, 0.08, 0.92]
        } else {
            [tint[0], tint[1], tint[2], 1.]
        };
        let half = r * 0.66;
        overlays.push(Overlay::Icon {
            rect: [centre[0] - half, centre[1] - half, half * 2., half * 2.],
            image: glyph(item.icon.glyph),
            color: [ink[0], ink[1], ink[2], ink[3] * fade],
        });
        drawn.push((item.entity.to_owned(), centre, r));
    }
}
