//! Match the selected box's projected silhouette, rather than counting scene colors.
use ara_ui::paint::Box2;
use serde::Serialize;

const SAMPLES_PER_EDGE: usize = 12;
const COLOR_TOLERANCE: u8 = 24;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Evidence {
    algorithm: &'static str,
    outline_colors_rgb: Option<[[u8; 3]; 2]>,
    color_tolerance: u8,
    radius_physical_pixels: i32,
    projected_hull: Vec<[f32; 2]>,
    projected_area_logical_pixels: f64,
    edges: Vec<Edge>,
    samples: usize,
    matched_samples: usize,
    matched_edges: usize,
    pub matches_geometry: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Edge {
    samples: usize,
    matched: usize,
}

/// `overlay::build` emits opaque accent; the native loop can append muted hover
/// at alpha 0.65. Premultiplied blending writes these bytes to the UNORM target.
pub(super) fn outline_colors(accent: [u8; 3], muted: [u8; 3]) -> [[u8; 3]; 2] {
    [
        accent,
        std::array::from_fn(|channel| {
            (f32::from(muted[channel]) * 0.65 + f32::from(accent[channel]) * 0.35).round() as u8
        }),
    ]
}

fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f64 {
    (f64::from(b[0]) - f64::from(a[0])) * (f64::from(c[1]) - f64::from(a[1]))
        - (f64::from(b[1]) - f64::from(a[1])) * (f64::from(c[0]) - f64::from(a[0]))
}
fn hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if points.iter().flatten().any(|value| !value.is_finite()) {
        return Vec::new();
    }
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    sorted.dedup();
    if sorted.len() < 3 {
        return Vec::new();
    }
    let mut lower: Vec<[f32; 2]> = Vec::new();
    for &point in &sorted {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.
        {
            lower.pop();
        }
        lower.push(point);
    }
    let mut upper: Vec<[f32; 2]> = Vec::new();
    for &point in sorted.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.
        {
            upper.pop();
        }
        upper.push(point);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The outline comes from renderer entity IDs and the current theme's accent token.
/// Require its continuous coverage on three independently projected box edges;
/// toolbar icons, isolated grid crossings and flat placeholders cannot satisfy it.
pub(super) fn inspect(
    image: &image::RgbaImage,
    rect: Box2,
    dpi: f32,
    points: &[[f32; 2]],
    colors: Option<[[u8; 3]; 2]>,
) -> Evidence {
    let mut evidence = Evidence {
        algorithm: "projected-selection-silhouette/1",
        outline_colors_rgb: colors,
        color_tolerance: COLOR_TOLERANCE,
        ..Default::default()
    };
    if !dpi.is_finite()
        || !(0.25..=8.).contains(&dpi)
        || ![rect.x, rect.y, rect.w, rect.h]
            .iter()
            .all(|value| value.is_finite())
        || rect.w <= 8.
        || rect.h <= 56.
    {
        return evidence;
    }
    let Some(colors) = colors else {
        return evidence;
    };
    evidence.projected_hull = hull(points);
    if evidence.projected_hull.len() < 3 {
        return evidence;
    }
    evidence.projected_area_logical_pixels = (0..evidence.projected_hull.len())
        .map(|index| {
            cross(
                [0., 0.],
                evidence.projected_hull[index],
                evidence.projected_hull[(index + 1) % evidence.projected_hull.len()],
            )
        })
        .sum::<f64>()
        .abs()
        * 0.5;
    if evidence.projected_area_logical_pixels < 16. {
        return evidence;
    }
    // Full-opacity outline pixels survive conversion exactly; allow antialiasing
    // around them and three physical pixels of projected/raster edge rounding.
    evidence.radius_physical_pixels = (2. * dpi).ceil() as i32 + 1;
    let radius = evidence.radius_physical_pixels;
    let visible = Box2 {
        x: rect.x + 4.,
        y: rect.y + 48.,
        w: rect.w - 8.,
        h: rect.h - 52.,
    };
    let n = evidence.projected_hull.len();
    for index in 0..n {
        let a = evidence.projected_hull[index];
        let b = evidence.projected_hull[(index + 1) % n];
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        if length * dpi < 16. {
            continue;
        }
        let normal = [-(b[1] - a[1]) / length, (b[0] - a[0]) / length];
        let mut edge = Edge {
            samples: 0,
            matched: 0,
        };
        for sample in 0..SAMPLES_PER_EDGE {
            let t = 0.1 + 0.8 * sample as f32 / (SAMPLES_PER_EDGE - 1) as f32;
            let point = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            if !visible.contains(point[0], point[1]) {
                continue;
            }
            let x = (point[0] * dpi).round() as i32;
            let y = (point[1] * dpi).round() as i32;
            if x < radius
                || y < radius
                || i64::from(x) + i64::from(radius) >= i64::from(image.width())
                || i64::from(y) + i64::from(radius) >= i64::from(image.height())
            {
                continue;
            }
            edge.samples += 1;
            let outline_at = |x: i32, y: i32| -> Option<bool> {
                if x < 0
                    || y < 0
                    || i64::from(x) >= i64::from(image.width())
                    || i64::from(y) >= i64::from(image.height())
                {
                    return None;
                }
                let rgba = image.get_pixel(x as u32, y as u32).0;
                Some(
                    rgba[3] >= 240
                        && colors.iter().any(|color| {
                            (0..3).all(|channel| {
                                rgba[channel].abs_diff(color[channel]) <= COLOR_TOLERANCE
                            })
                        }),
                )
            };
            let outline = (-radius..=radius)
                .any(|dy| (-radius..=radius).any(|dx| outline_at(x + dx, y + dy) == Some(true)));
            // A silhouette is a narrow edge, not a flat accent-colored placeholder.
            let contrast = [-1., 1.].into_iter().all(|side| {
                let gap = (radius + 4) as f32 * side;
                outline_at(
                    x + (normal[0] * gap).round() as i32,
                    y + (normal[1] * gap).round() as i32,
                ) == Some(false)
            });
            let matched = outline && contrast;
            edge.matched += usize::from(matched);
        }
        evidence.samples += edge.samples;
        evidence.matched_samples += edge.matched;
        evidence.matched_edges +=
            usize::from(edge.samples >= 4 && edge.matched * 3 >= edge.samples * 2);
        evidence.edges.push(edge);
    }
    evidence.matches_geometry = evidence.samples >= 24
        && evidence.matched_edges >= 3
        && evidence.matched_samples * 3 >= evidence.samples * 2;
    evidence
}

#[cfg(test)]
mod tests;
