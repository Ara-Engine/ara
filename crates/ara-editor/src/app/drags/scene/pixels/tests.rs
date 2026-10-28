//! Flat shaded geometry is valid; placeholders and unrelated grid strokes are not.
use super::*;
const ACCENT: [u8; 3] = [72, 100, 255];
const MUTED: [u8; 3] = [129; 3];
fn colors() -> Option<[[u8; 3]; 2]> {
    Some(outline_colors(ACCENT, MUTED))
}

fn subject(rect: Box2) -> Vec<[f32; 2]> {
    vec![
        [rect.x + rect.w * 0.25, rect.y + rect.h * 0.3],
        [rect.x + rect.w * 0.75, rect.y + rect.h * 0.3],
        [rect.x + rect.w * 0.7, rect.y + rect.h * 0.83],
        [rect.x + rect.w * 0.28, rect.y + rect.h * 0.83],
    ]
}
fn image(dpi: f32) -> image::RgbaImage {
    image::RgbaImage::from_pixel(
        (1000. * dpi) as u32,
        (800. * dpi) as u32,
        image::Rgba([180, 180, 180, 255]),
    )
}
fn paint_box(image: &mut image::RgbaImage, points: &[[f32; 2]], dpi: f32, color: [u8; 3]) {
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let p = [(x as f32 + 0.5) / dpi, (y as f32 + 0.5) / dpi];
        let inside = (0..points.len())
            .all(|index| cross(points[index], points[(index + 1) % points.len()], p) >= 0.);
        if inside {
            *pixel = image::Rgba([196, 196, 196, 255]);
        }
        if (0..points.len()).any(|index| {
            let a = glam::Vec2::from_array(points[index]);
            let b = glam::Vec2::from_array(points[(index + 1) % points.len()]);
            let p = glam::Vec2::from_array(p);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0., 1.);
            p.distance(a + ab * t) * dpi <= 1.5 * dpi
        }) {
            *pixel = image::Rgba([color[0], color[1], color[2], 255]);
        }
    }
}

#[test]
fn flat_shaded_box_uses_actual_outline_geometry_at_wide_and_portrait_dpi() {
    for rect in [
        Box2 {
            x: 20.,
            y: 30.,
            w: 900.,
            h: 300.,
        },
        Box2 {
            x: 100.,
            y: 20.,
            w: 360.,
            h: 650.,
        },
    ] {
        for dpi in [1., 1.25, 2.] {
            let points = subject(rect);
            let mut pixels = image(dpi);
            paint_box(&mut pixels, &points, dpi, ACCENT);
            let evidence = inspect(&pixels, rect, dpi, &points, colors());
            assert!(
                evidence.matches_geometry,
                "{}",
                serde_json::to_string(&evidence).unwrap()
            );
            assert_eq!(evidence.matched_edges, 4);
            assert_eq!(evidence.matched_samples, evidence.samples);
        }
    }
}

#[test]
fn black_uniform_and_grid_only_images_do_not_establish_rendered_geometry() {
    let rect = Box2 {
        x: 20.,
        y: 30.,
        w: 900.,
        h: 650.,
    };
    let points = subject(rect);
    for background in [
        [0, 0, 0, 255],
        [196, 196, 196, 255],
        [ACCENT[0], ACCENT[1], ACCENT[2], 255],
    ] {
        let pixels = image::RgbaImage::from_pixel(1000, 800, image::Rgba(background));
        let evidence = inspect(&pixels, rect, 1., &points, colors());
        // A uniform accent surface must not masquerade as a silhouette either.
        assert!(!evidence.matches_geometry);
    }
    let mut pixels = image(1.);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        if x % 24 == 0 || y % 24 == 0 {
            *pixel = image::Rgba([ACCENT[0], ACCENT[1], ACCENT[2], 255]);
        }
    }
    assert!(!inspect(&pixels, rect, 1., &points, colors()).matches_geometry);
}

#[test]
fn misplaced_outline_invalid_geometry_and_missing_theme_cannot_pass() {
    let rect = Box2 {
        x: 20.,
        y: 30.,
        w: 900.,
        h: 650.,
    };
    let points = subject(rect);
    let mut pixels = image(1.);
    paint_box(&mut pixels, &points, 1., ACCENT);
    let displaced: Vec<_> = points.iter().map(|p| [p[0] + 35., p[1] + 21.]).collect();
    assert!(!inspect(&pixels, rect, 1., &displaced, colors()).matches_geometry);
    assert!(!inspect(&pixels, rect, f32::NAN, &points, colors()).matches_geometry);
    assert!(!inspect(&pixels, rect, 1., &[[f32::NAN, 2.]; 4], colors()).matches_geometry);
    assert!(!inspect(&pixels, rect, 1., &[[1., 2.]; 4], colors()).matches_geometry);
    assert!(!inspect(&pixels, rect, 1., &points, None).matches_geometry);
}

#[test]
fn hover_composite_matches_the_producer_but_a_uniform_composite_does_not() {
    assert_eq!(outline_colors([52, 107, 241], MUTED)[1], [102, 121, 168]);
    let rect = Box2 {
        x: 20.,
        y: 30.,
        w: 900.,
        h: 650.,
    };
    let points = subject(rect);
    let composite = outline_colors(ACCENT, MUTED)[1];
    let mut pixels = image(1.);
    paint_box(&mut pixels, &points, 1., composite);
    let evidence = inspect(&pixels, rect, 1., &points, colors());
    assert!(
        evidence.matches_geometry,
        "{}",
        serde_json::to_string(&evidence).unwrap()
    );
    assert_eq!(evidence.matched_edges, 4);
    let uniform = image::RgbaImage::from_pixel(
        1000,
        800,
        image::Rgba([composite[0], composite[1], composite[2], 255]),
    );
    assert!(!inspect(&uniform, rect, 1., &points, colors()).matches_geometry);
}
