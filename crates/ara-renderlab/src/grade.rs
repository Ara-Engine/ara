//! Ara's tone curve and colour grade as Renderlab's colour lookup table. Ara grades in
//! scene-linear light before its tone curve (white balance, contrast about 18% grey,
//! saturation), maps through the look's curve (PBR Neutral, Hill's Filmic, AgX and its
//! looks, or the ACES fit), and grades in display space after it (lift, gamma, gain).
//! Renderlab applies its LUT to the display-linear colour after its own curve (the ACES
//! fit), so each LUT entry undoes that curve, grades and maps as Ara does: the image takes
//! Ara's curve, not only its middle grey. Scene values past where Renderlab's curve
//! saturates (about 9 after exposure) all read as its brightest entry. Renderlab's own
//! white balance, contrast and saturation stay neutral.
use ara_render_fidelity::{
    colour::{ToneMapper, tone_map},
    environment::FrameEnvironment,
};
use renderlab::engine::ColourLut;

/// Entries per axis: the filmic curve is smooth, and 33 is the common grading size.
const SIZE: u32 = 33;

/// Renderlab's `Tonemapper::Filmic` (Narkowicz's fit), clamped to [0, 1].
#[cfg(test)]
fn filmic(x: f32) -> f32 {
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0., 1.)
}

/// The scene-linear value `filmic` maps to `y`: the positive root of
/// (2.43y - 2.51)x² + (0.59y - 0.03)x + 0.14y = 0.
fn inverse_filmic(y: f32) -> f32 {
    let y = y.clamp(0., 0.999);
    let (a, b, c) = (2.43 * y - 2.51, 0.59 * y - 0.03, 0.14 * y);
    let root = (b * b - 4. * a * c).max(0.).sqrt();
    ((-b - root) / (2. * a)).max(0.)
}

/// Whether the look grades or takes another curve than Renderlab's, so Renderlab applies
/// the table (`lut`).
pub(crate) fn lut_needed(look: &FrameEnvironment) -> bool {
    !neutral(look)
}

/// Whether the look neither grades nor departs from Renderlab's curve: no table.
fn neutral(look: &FrameEnvironment) -> bool {
    let g = &look.grading;
    let [lift, gamma, gain] = look.display;
    look.tone_mapper == ToneMapper::AcesFit
        && g.white_balance == 6500.
        && g.contrast == 1.
        && g.saturation == 1.
        && lift == [0.; 3]
        && gamma == [1.; 3]
        && gain == [1.; 3]
}

/// The table for `look`, or None for a neutral look.
pub(crate) fn lut(look: &FrameEnvironment) -> Option<ColourLut> {
    if neutral(look) {
        return None;
    }
    let g = &look.grading;
    let [lift, gamma, gain] = look.display;
    let mut rgb = Vec::with_capacity((SIZE * SIZE * SIZE) as usize);
    let step = 1. / (SIZE - 1) as f32;
    for b in 0..SIZE {
        for gr in 0..SIZE {
            for r in 0..SIZE {
                // Ara's own scene-linear grade, then its curve.
                let scene = [r, gr, b].map(|v| inverse_filmic(v as f32 * step));
                let graded = g.apply(glam30::Vec3::from_array(scene));
                let c = tone_map(graded, look.tone_mapper).to_array();
                // Display grade after the curve: lift fades out toward white.
                rgb.push(std::array::from_fn(|i| {
                    let x = c[i];
                    let lifted = (x + lift[i] * (1. - x)).max(0.);
                    (gain[i] * lifted.powf(1. / gamma[i].max(0.01))).clamp(0., 1.)
                }));
            }
        }
    }
    Some(ColourLut { size: SIZE, rgb })
}

/// Identity of `look`'s grade, to rebuild the table only when it changes.
pub(crate) fn key(look: &FrameEnvironment) -> u64 {
    let g = &look.grading;
    let values = [&g.white_balance, &g.contrast, &g.saturation]
        .into_iter()
        .chain(look.display.iter().flatten());
    let seed = 0xcbf2_9ce4_8422_2325u64 ^ look.tone_mapper as u64;
    values.fold(seed, |h, v| {
        v.to_bits().to_le_bytes().iter().fold(h, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filmic_curve_inverts() {
        for y in [0.01f32, 0.18, 0.5, 0.9] {
            assert!((filmic(inverse_filmic(y)) - y).abs() < 1e-4, "{y}");
        }
    }

    #[test]
    fn a_neutral_look_has_no_table_and_a_graded_one_does() {
        let mut look = FrameEnvironment::resolve(&Default::default(), glam30::Vec3::ZERO, 1.);
        look.tone_mapper = ToneMapper::AcesFit;
        assert!(lut(&look).is_none());
        look.grading.saturation = 0.;
        let table = lut(&look).expect("graded");
        // Greyscale: every entry's channels agree.
        let pure_red = table.rgb[(SIZE - 1) as usize];
        assert!(
            (pure_red[0] - pure_red[1]).abs() < 1e-4 && (pure_red[1] - pure_red[2]).abs() < 1e-4
        );
        look.grading.saturation = 1.;
        look.display[2] = [2., 1., 1.];
        // Gain on red: mid grey turns redder.
        let mid = (SIZE / 2) as usize;
        let at = mid + mid * SIZE as usize + mid * (SIZE * SIZE) as usize;
        let graded = lut(&look).expect("graded").rgb[at];
        assert!(graded[0] > graded[1] * 1.5, "{graded:?}");
    }

    /// Ara's other curves land through the table: middle grey shown by Renderlab's curve
    /// reads as each curve shows it (Hill's Filmic and AgX keep 18% at about 18%), and a
    /// bright value as Ara's curve compresses it, not Renderlab's.
    #[test]
    fn the_table_carries_aras_curve() {
        let mut look = FrameEnvironment::resolve(&Default::default(), glam30::Vec3::ZERO, 1.);
        for mapper in [ToneMapper::Filmic, ToneMapper::Agx, ToneMapper::PbrNeutral] {
            look.tone_mapper = mapper;
            let table = lut(&look).expect("another curve needs the table");
            let at = |v: f32| {
                // The table entry nearest Renderlab's display value for scene value v.
                let i = (filmic(v) * (SIZE - 1) as f32).round() as usize;
                let scene = inverse_filmic(i as f32 / (SIZE - 1) as f32);
                let want = tone_map(glam30::Vec3::splat(scene), mapper).x;
                (
                    table.rgb[i + i * SIZE as usize + i * (SIZE * SIZE) as usize][0],
                    want,
                )
            };
            for v in [0.18f32, 2.] {
                let (got, want) = at(v);
                assert!((got - want).abs() < 1e-3, "{mapper:?} {v}: {got} vs {want}");
            }
        }
    }
}
