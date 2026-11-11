//! Ara's tone curve and colour grade as Renderlab's colour lookup table. Ara grades in
//! scene-linear light before its tone curve (white balance, contrast about 18% grey,
//! saturation), maps through the look's curve (PBR Neutral, Hill's Filmic, AgX and its
//! looks, or the ACES fit), and grades in display space after it (lift, gamma, gain).
//! Renderlab applies its LUT to the display-linear colour after its own curve (the ACES
//! fit), so each LUT entry undoes that curve, grades and maps as Ara does: the image takes
//! Ara's curve, not only its middle grey. A look's authored table (`Authored`) is applied
//! last, as Ara applies it: to the sRGB-encoded colour, blended by its contribution. Scene
//! values past where Renderlab's curve saturates (about 9 after exposure) all read as its
//! brightest entry. Renderlab's own
//! white balance, contrast and saturation stay neutral.
use ara_render_fidelity::{
    colour::{ToneMapper, tone_map},
    environment::FrameEnvironment,
};
use ara_runtime_render::ExternalView;
use renderlab::engine::ColourLut;

/// A look's authored colour lookup table (`FrameEnvironment::lut` into the frame's
/// `SceneLook::luts`) and the share of it applied.
#[derive(Clone, Copy)]
pub(crate) struct Authored<'a> {
    table: &'a ara_components::post::ColorLut,
    share: f32,
}

/// The authored table the frame's look applies, if any (as Ara's `apply_lut`).
pub(crate) fn authored<'a>(view: &ExternalView<'a>) -> Option<Authored<'a>> {
    let share = view.look.lut_contribution.clamp(0., 1.);
    view.look
        .lut
        .and_then(|i| view.frame.look.luts.get(i as usize))
        .map(|(_, table)| Authored {
            table: table.as_ref(),
            share,
        })
        .filter(|a| a.share > 0. && a.table.size >= 2)
}

/// Ara's sRGB encoding (`srgb`, shaders/look.slangh) and its inverse.
fn srgb(x: f32) -> f32 {
    if x < 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.max(0.).powf(1. / 2.4) - 0.055
    }
}
fn linear(s: f32) -> f32 {
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

impl Authored<'_> {
    /// The table at an sRGB-encoded colour, as Ara's `colourLookup`: trilinear over its
    /// size³ RGBA8 texels in strip order (texel g·n² + b·n + r, red in the low byte).
    fn lookup(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.table.size as usize;
        let at = c.map(|v| v.clamp(0., 1.) * (n - 1) as f32);
        let base = at.map(|v| (v as usize).min(n - 2));
        let t: [f32; 3] = std::array::from_fn(|i| at[i] - base[i] as f32);
        let mut sum = [0f32; 3];
        for k in 0..8 {
            let o = [k & 1, (k >> 1) & 1, k >> 2];
            let q: [usize; 3] = std::array::from_fn(|i| base[i] + o[i]);
            let texel = self
                .table
                .texels
                .get(q[1] * n * n + q[2] * n + q[0])
                .copied()
                .unwrap_or(0);
            let w: f32 = (0..3)
                .map(|i| if o[i] == 1 { t[i] } else { 1. - t[i] })
                .product();
            for (c, s) in sum.iter_mut().enumerate() {
                *s += f32::from((texel >> (8 * c)) as u8) / 255. * w;
            }
        }
        sum
    }
}

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
pub(crate) fn lut_needed(look: &FrameEnvironment, authored: Option<Authored<'_>>) -> bool {
    !neutral(look) || authored.is_some()
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

/// The table for `look` and its authored table, or None for a neutral look without one.
pub(crate) fn lut(look: &FrameEnvironment, authored: Option<Authored<'_>>) -> Option<ColourLut> {
    if !lut_needed(look, authored) {
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
                // Display grade after the curve, as Ara's liftGammaGain: lift fades out
                // toward white, gain scales, then gamma.
                let display: [f32; 3] = std::array::from_fn(|i| {
                    let x = c[i].clamp(0., 1.);
                    let lifted = (gain[i] * (x + lift[i] * (1. - x))).clamp(0., 1.);
                    lifted.powf(1. / gamma[i].max(1e-3))
                });
                // The authored table last, on the sRGB-encoded colour.
                rgb.push(match authored {
                    Some(a) => {
                        let encoded = display.map(srgb);
                        let looked = a.lookup(encoded);
                        std::array::from_fn(|i| {
                            linear(encoded[i] + (looked[i] - encoded[i]) * a.share).clamp(0., 1.)
                        })
                    }
                    None => display,
                });
            }
        }
    }
    Some(ColourLut { size: SIZE, rgb })
}

/// Identity of `look`'s grade and authored table, to rebuild the table only when it
/// changes.
pub(crate) fn key(look: &FrameEnvironment, authored: Option<Authored<'_>>) -> u64 {
    let g = &look.grading;
    let share = authored.map_or(0., |a| a.share);
    let values = [&g.white_balance, &g.contrast, &g.saturation, &share]
        .into_iter()
        .chain(look.display.iter().flatten());
    // The table's identity: the frame shares one allocation per authored table.
    let table = authored.map_or(0, |a| a.table as *const _ as usize as u64);
    let seed = 0xcbf2_9ce4_8422_2325u64 ^ look.tone_mapper as u64 ^ table.rotate_left(17);
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
        assert!(lut(&look, None).is_none());
        look.grading.saturation = 0.;
        let table = lut(&look, None).expect("graded");
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
        let graded = lut(&look, None).expect("graded").rgb[at];
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
            let table = lut(&look, None).expect("another curve needs the table");
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

    /// Ara's authored table is applied last on the sRGB-encoded colour: an identity table
    /// leaves the image as it was, an inverting one at half contribution lands on mid
    /// grey everywhere.
    #[test]
    fn the_authored_table_applies_last_in_srgb() {
        let n = 4u32;
        let table = |f: &dyn Fn([u32; 3]) -> [u32; 3]| ara_components::post::ColorLut {
            size: n,
            texels: (0..n * n * n)
                .map(|i| {
                    // Strip order: texel g·n² + b·n + r.
                    let (g, b, r) = (i / (n * n), (i / n) % n, i % n);
                    let [r, g, b] = f([r, g, b]).map(|v| v * 255 / (n - 1));
                    r | g << 8 | b << 16 | 255 << 24
                })
                .collect(),
        };
        let mut look = FrameEnvironment::resolve(&Default::default(), glam30::Vec3::ZERO, 1.);
        look.tone_mapper = ToneMapper::AcesFit;
        let identity = table(&|c| c);
        let plain = Authored {
            table: &identity,
            share: 1.,
        };
        let lut = lut(&look, Some(plain)).expect("an authored table needs the table");
        let mid = (SIZE / 2) as usize;
        let at = mid + mid * SIZE as usize + mid * (SIZE * SIZE) as usize;
        let v = mid as f32 / (SIZE - 1) as f32;
        assert!((lut.rgb[at][0] - v).abs() < 2e-3, "{:?}", lut.rgb[at]);
        let inverse = table(&|c| c.map(|v| n - 1 - v));
        let half = Authored {
            table: &inverse,
            share: 0.5,
        };
        let lut = super::lut(&look, Some(half)).expect("table");
        for i in [0, at, lut.rgb.len() - 1] {
            assert!(
                (srgb(lut.rgb[i][0]) - 0.5).abs() < 2e-3,
                "{i}: {:?}",
                lut.rgb[i]
            );
        }
    }
}
