//! Colour maths for the picker and swatches: the exact sRGB transfer functions, HSV,
//! hex text, HDR intensity in stops and blackbody (Kelvin) colours.
//!
//! Documents store linear RGB(A); the picker edits in display (sRGB) space, as Unity,
//! Unreal and Godot do, and converts with the piecewise sRGB curve (never gamma 2.2).

/// sRGB-encoded channel (0–1) to linear light.
pub(crate) fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
/// Linear-light channel to sRGB encoding; values above 1 extend the curve.
pub(crate) fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1. / 2.4) - 0.055
    }
}
/// A linear colour as display bytes, clamped (UI swatches, hex text).
pub(crate) fn display_bytes(linear: [f64; 3]) -> [u8; 3] {
    linear.map(|c| (linear_to_srgb(c.clamp(0., 1.)) * 255.).round() as u8)
}
/// A linear RGBA value as a UI colour (sRGB bytes, straight alpha).
pub(crate) fn ui_color(linear: [f64; 4]) -> ara_ui::style::Color {
    let [r, g, b] = display_bytes([linear[0], linear[1], linear[2]]);
    ara_ui::style::Color(r, g, b, (linear[3].clamp(0., 1.) * 255.).round() as u8)
}
/// Display-space RGB (0–1) to hue (0–1, red at 0), saturation and value.
pub(crate) fn rgb_to_hsv([r, g, b]: [f64; 3]) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f64::EPSILON {
        0.
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.) / 6.
    } else if max == g {
        ((b - r) / delta + 2.) / 6.
    } else {
        ((r - g) / delta + 4.) / 6.
    };
    let saturation = if max <= f64::EPSILON { 0. } else { delta / max };
    [hue, saturation, max]
}
/// Hue (0–1, wrapping), saturation and value to display-space RGB.
pub(crate) fn hsv_to_rgb([h, s, v]: [f64; 3]) -> [f64; 3] {
    let h = h.rem_euclid(1.) * 6.;
    let channel = |n: f64| {
        let k = (n + h) % 6.;
        v - v * s * k.min(4. - k).clamp(0., 1.)
    };
    [channel(5.), channel(3.), channel(1.)]
}
/// `RGB`, `RRGGBB` or `RRGGBBAA`, with or without `#`, as 0–1 channels; alpha is
/// `None` when the text has none.
pub(crate) fn parse_hex(text: &str) -> Option<([f64; 3], Option<f64>)> {
    let hex = text.trim().trim_start_matches('#');
    if !hex.is_ascii() {
        return None;
    }
    let digits: Vec<u8> = match hex.len() {
        3 => hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| (d * 17) as u8))
            .collect::<Option<_>>()?,
        6 | 8 => (0..hex.len() / 2)
            .map(|n| u8::from_str_radix(&hex[n * 2..n * 2 + 2], 16).ok())
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let unit = |n: usize| f64::from(digits[n]) / 255.;
    Some(([unit(0), unit(1), unit(2)], digits.get(3).map(|_| unit(3))))
}
/// 0–1 channels as `RRGGBB`, or `RRGGBBAA` when `alpha` is given.
pub(crate) fn format_hex(rgb: [f64; 3], alpha: Option<f64>) -> String {
    let byte = |c: f64| (c.clamp(0., 1.) * 255.).round() as u8;
    let mut out = format!(
        "{:02X}{:02X}{:02X}",
        byte(rgb[0]),
        byte(rgb[1]),
        byte(rgb[2])
    );
    if let Some(a) = alpha {
        out.push_str(&format!("{:02X}", byte(a)));
    }
    out
}
/// An HDR linear colour as a base colour (brightest channel at most 1) and an
/// intensity in stops: `linear = base · 2^stops`. LDR colours have zero stops.
pub(crate) fn split_hdr(linear: [f64; 3]) -> ([f64; 3], f64) {
    let max = linear[0].max(linear[1]).max(linear[2]);
    if max <= 1. {
        return (linear, 0.);
    }
    let stops = max.log2();
    (linear.map(|c| c / max), stops)
}
/// The lowest and highest blackbody temperatures the picker offers, in Kelvin.
pub(crate) const KELVIN: (f64, f64) = (1000., 15000.);
/// Named blackbody presets: candle flame to clear blue sky.
pub(crate) const KELVIN_PRESETS: [(&str, f64); 8] = [
    ("Candle", 1900.),
    ("Tungsten", 2700.),
    ("Halogen", 3200.),
    ("Moonlight", 4100.),
    ("Daylight", 5600.),
    ("Overcast", 6500.),
    ("Shade", 7500.),
    ("Blue sky", 10000.),
];
/// The linear sRGB colour of a blackbody at `kelvin`, brightest channel 1: Krystek's
/// Planckian-locus fit in CIE 1960 (u, v), through XYZ to linear Rec. 709.
pub(crate) fn kelvin_to_linear(kelvin: f64) -> [f64; 3] {
    let t = kelvin.clamp(KELVIN.0, KELVIN.1);
    let u = (0.860_117_757 + 1.541_182_54e-4 * t + 1.286_412_12e-7 * t * t)
        / (1. + 8.424_202_35e-4 * t + 7.081_451_63e-7 * t * t);
    let v = (0.317_398_726 + 4.228_062_45e-5 * t + 4.204_816_91e-8 * t * t)
        / (1. - 2.897_418_16e-5 * t + 1.614_560_53e-7 * t * t);
    let d = 2. * u - 8. * v + 4.;
    let (x, y) = (3. * u / d, 2. * v / d);
    let (cx, cz) = (x / y, (1. - x - y) / y);
    let rgb = [
        3.240_454_2 * cx - 1.537_138_5 - 0.498_531_4 * cz,
        -0.969_266 * cx + 1.876_010_8 + 0.041_556 * cz,
        0.055_643_4 * cx - 0.204_025_9 + 1.057_225_2 * cz,
    ]
    .map(|c: f64| c.max(0.));
    let max = rgb[0].max(rgb[1]).max(rgb[2]).max(f64::EPSILON);
    rgb.map(|c| c / max)
}
/// The blackbody temperature whose colour `linear` is (up to brightness), when it is
/// one within a small tolerance; colours off the Planckian locus have none.
pub(crate) fn linear_to_kelvin(linear: [f64; 3]) -> Option<f64> {
    let max = linear[0].max(linear[1]).max(linear[2]);
    if max <= f64::EPSILON {
        return None;
    }
    let target = linear.map(|c| c / max);
    let error = |k: f64| {
        let c = kelvin_to_linear(k);
        (0..3).map(|i| (c[i] - target[i]).abs()).fold(0., f64::max)
    };
    let coarse = (0..=280)
        .map(|n| KELVIN.0 + f64::from(n) * 50.)
        .min_by(|a, b| error(*a).total_cmp(&error(*b)))?;
    let fine = (-50..=50)
        .map(|n| (coarse + f64::from(n)).clamp(KELVIN.0, KELVIN.1))
        .min_by(|a, b| error(*a).total_cmp(&error(*b)))?;
    (error(fine) < 0.004).then_some(fine)
}
/// Text colour (CSS hex) readable on a linear colour, clipped at 1: dark on light
/// colours, light on dark ones (they contrast equally at a luminance of about 0.18).
pub(crate) fn contrast_text(linear: [f64; 3]) -> &'static str {
    let [r, g, b] = linear.map(|c| c.clamp(0., 1.));
    if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.18 {
        "#101010"
    } else {
        "#F2F2F2"
    }
}
/// Rounds a stored channel so documents stay stable (1e-5 is below an 8-bit step).
pub(crate) fn stable(c: f64) -> f64 {
    (c * 100_000.).round() / 100_000.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips_every_byte() {
        for byte in 0..=255u8 {
            let encoded = f64::from(byte) / 255.;
            let back = linear_to_srgb(srgb_to_linear(encoded));
            assert!((back - encoded).abs() < 1e-9, "{byte}");
        }
        // The piecewise curve, not gamma 2.2: mid grey is 0.2140, not 0.2176.
        assert!((srgb_to_linear(0.5) - 0.214_041).abs() < 1e-6);
        assert_eq!(display_bytes([0.214_041, 0., 1.]), [128, 0, 255]);
    }

    #[test]
    fn hsv_round_trips_and_keeps_hue_order() {
        for rgb in [
            [1., 0., 0.],
            [0.2, 0.6, 0.4],
            [0.9, 0.9, 0.1],
            [0.3, 0.1, 0.8],
        ] {
            let back = hsv_to_rgb(rgb_to_hsv(rgb));
            for i in 0..3 {
                assert!((back[i] - rgb[i]).abs() < 1e-12, "{rgb:?} -> {back:?}");
            }
        }
        assert_eq!(rgb_to_hsv([0., 1., 0.])[0], 1. / 3.);
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-12);
        assert!(near(hsv_to_rgb([2. / 3., 1., 1.]), [0., 0., 1.]));
        // Hue wraps: 1.0 is red again.
        assert!(near(hsv_to_rgb([1., 1., 1.]), [1., 0., 0.]));
    }

    #[test]
    fn hex_accepts_short_long_and_alpha_forms() {
        assert_eq!(parse_hex("#fff"), Some(([1., 1., 1.], None)));
        let (rgb, alpha) = parse_hex("BCAA7280").expect("eight digits");
        assert_eq!(format_hex(rgb, alpha), "BCAA7280");
        assert_eq!(format_hex(rgb, None), "BCAA72");
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("zzzzzz"), None);
        assert_eq!(parse_hex("0é000"), None);
    }

    #[test]
    fn hdr_splits_into_base_and_stops() {
        let (base, stops) = split_hdr([4., 2., 1.]);
        assert_eq!((base, stops), ([1., 0.5, 0.25], 2.));
        assert_eq!(split_hdr([0.5, 0.25, 1.]), ([0.5, 0.25, 1.], 0.));
    }

    #[test]
    fn blackbody_colours_are_warm_to_cool_and_invert() {
        let candle = kelvin_to_linear(1900.);
        let sky = kelvin_to_linear(10000.);
        assert_eq!(candle[0], 1.);
        assert!(candle[2] < 0.1, "{candle:?}");
        assert_eq!(sky[2], 1.);
        // D65 white is close to 6500 K on the locus.
        let d65 = kelvin_to_linear(6504.);
        assert!(d65.iter().all(|c| *c > 0.95), "{d65:?}");
        for kelvin in [1900., 3200., 5600., 10000.] {
            let found =
                linear_to_kelvin(kelvin_to_linear(kelvin).map(|c| c * 0.4)).expect("on the locus");
            assert!((found - kelvin).abs() <= 25., "{kelvin} -> {found}");
        }
        assert_eq!(linear_to_kelvin([0., 1., 0.]), None);
    }
}
