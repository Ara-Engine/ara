//! IES photometric profiles (IESNA LM-63, 1986 to 2019 revisions, type C photometry)
//! as light profiles (`Light.profile`). The candela web is resampled onto a fixed grid
//! normalised to its peak; the artifact `profile.bin` is [`ara_asset_format`]-free and
//! read by `ara_runtime::light_profile`:
//!
//! `ARAIES01`, then little-endian u32 vertical and horizontal sample counts, f32 peak
//! candela (multipliers and ballast factors applied) and f32 total lumens, then
//! vertical x horizontal f32 intensities over the peak: horizontal-major, vertical
//! angle 0 (nadir) to 180 degrees inclusive, azimuth 0 to 360 degrees exclusive (one
//! column for rotationally symmetric profiles).
use ara_asset::{
    Artifact, ArtifactKind, Error, ImportContext, ImportInput, ImportOutput, Importer,
};
use std::f64::consts::PI;

/// Vertical samples, nadir to zenith inclusive.
pub const VERTICAL: usize = 64;
/// Azimuth samples of a profile that is not rotationally symmetric.
pub const HORIZONTAL: usize = 32;

/// The `ara.ies` importer.
pub struct IesImporter;

/// A parsed LM-63 candela web (type C).
#[derive(Debug, Clone, PartialEq)]
pub struct Photometry {
    /// Vertical angles in degrees, ascending.
    pub vertical: Vec<f64>,
    /// Horizontal angles in degrees, ascending.
    pub horizontal: Vec<f64>,
    /// Candela per horizontal angle, then per vertical angle, multipliers applied.
    pub candela: Vec<Vec<f64>>,
}

impl Photometry {
    /// Parses an LM-63 file. Tilt data (`TILT=INCLUDE`) is skipped; tilt files are
    /// ignored (the profile is measured upright).
    pub fn parse(text: &str) -> Result<Self, Error> {
        let invalid = |why: &str| Error::Invalid(format!("IES profile: {why}"));
        let mut lines = text.lines();
        let tilt = lines
            .by_ref()
            .find(|line| line.trim_start().to_ascii_uppercase().starts_with("TILT="))
            .ok_or_else(|| invalid("no TILT= line"))?;
        let rest: String = lines.collect::<Vec<_>>().join(" ");
        let mut numbers = rest
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .map(|t| {
                t.parse::<f64>()
                    .map_err(|_| invalid(&format!("`{t}` is not a number")))
            });
        let mut next = || {
            numbers
                .next()
                .unwrap_or_else(|| Err(invalid("the file ends early")))
        };
        if tilt.trim().to_ascii_uppercase().ends_with("INCLUDE") {
            let _geometry = next()?;
            let count = next()? as usize;
            for _ in 0..count * 2 {
                next()?;
            }
        }
        let _lamps = next()?;
        let _lumens_per_lamp = next()?;
        let multiplier = next()?;
        let (vertical_count, horizontal_count) = (next()? as usize, next()? as usize);
        let photometric_type = next()? as u32;
        let (_units, _width, _length, _height) = (next()?, next()?, next()?, next()?);
        let (ballast, lamp_factor, _watts) = (next()?, next()?, next()?);
        if photometric_type != 1 {
            return Err(invalid(
                "only type C photometry (luminaires) is supported; types A and B are not",
            ));
        }
        if vertical_count < 2 || horizontal_count < 1 || vertical_count * horizontal_count > 1 << 20
        {
            return Err(invalid("the angle counts are out of range"));
        }
        let vertical = (0..vertical_count)
            .map(|_| next())
            .collect::<Result<Vec<_>, _>>()?;
        let horizontal = (0..horizontal_count)
            .map(|_| next())
            .collect::<Result<Vec<_>, _>>()?;
        let scale = multiplier * ballast * if lamp_factor > 0. { lamp_factor } else { 1. };
        let candela = (0..horizontal_count)
            .map(|_| {
                (0..vertical_count)
                    .map(|_| next().map(|c| (c * scale).max(0.)))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ascending = |a: &[f64]| a.windows(2).all(|w| w[0] < w[1]);
        if !ascending(&vertical) || !ascending(&horizontal) {
            return Err(invalid("angles must ascend"));
        }
        Ok(Self {
            vertical,
            horizontal,
            candela,
        })
    }

    /// Candela towards vertical angle `theta` (degrees from nadir) at azimuth `phi`
    /// (degrees), with the file's horizontal symmetry (one plane, a quadrant, a half
    /// or the full circle) and zero outside its vertical range.
    pub fn candela(&self, theta: f64, phi: f64) -> f64 {
        let (first, last) = (
            self.horizontal[0],
            self.horizontal[self.horizontal.len() - 1],
        );
        let mut phi = phi.rem_euclid(360.);
        if self.horizontal.len() > 1 {
            if last - first <= 90. {
                phi = if phi > 180. { 360. - phi } else { phi };
                phi = if phi > 90. { 180. - phi } else { phi };
            } else if last - first <= 180. {
                phi = if phi > 180. { 360. - phi } else { phi };
            }
            phi = phi.clamp(first, last);
        }
        let column = |h: usize| interpolate(&self.vertical, &self.candela[h], theta);
        if self.horizontal.len() == 1 {
            return column(0);
        }
        let h = self
            .horizontal
            .partition_point(|&a| a <= phi)
            .clamp(1, self.horizontal.len() - 1);
        let (a, b) = (self.horizontal[h - 1], self.horizontal[h]);
        let t = ((phi - a) / (b - a)).clamp(0., 1.);
        column(h - 1) * (1. - t) + column(h) * t
    }
}

/// Linear interpolation of `values` at `x` over ascending `keys`; zero outside them.
fn interpolate(keys: &[f64], values: &[f64], x: f64) -> f64 {
    if x < keys[0] - 1e-9 || x > keys[keys.len() - 1] + 1e-9 {
        return 0.;
    }
    let i = keys.partition_point(|&k| k <= x).clamp(1, keys.len() - 1);
    let t = ((x - keys[i - 1]) / (keys[i] - keys[i - 1])).clamp(0., 1.);
    values[i - 1] * (1. - t) + values[i] * t
}

/// The resampled profile: (vertical, horizontal) sample counts, peak candela, total
/// lumens and the normalised grid (see the module docs).
pub fn resample(photometry: &Photometry) -> (usize, usize, f64, f64, Vec<f32>) {
    let horizontal = if photometry.horizontal.len() == 1 {
        1
    } else {
        HORIZONTAL
    };
    let mut grid = Vec::with_capacity(VERTICAL * horizontal);
    for h in 0..horizontal {
        let phi = h as f64 * 360. / horizontal as f64;
        for v in 0..VERTICAL {
            grid.push(photometry.candela(v as f64 * 180. / (VERTICAL - 1) as f64, phi));
        }
    }
    let peak = grid.iter().copied().fold(0., f64::max);
    // Flux: candela over the sphere (midpoint rule on a finer grid).
    let (steps, rings) = (720usize, 360usize);
    let mut lumens = 0.;
    for i in 0..rings {
        let theta = (i as f64 + 0.5) / rings as f64 * 180.;
        let ring = (theta.to_radians()).sin() * (PI / rings as f64) * (2. * PI / steps as f64);
        for j in 0..steps {
            lumens += photometry.candela(theta, (j as f64 + 0.5) / steps as f64 * 360.) * ring;
        }
    }
    let normalised = grid
        .iter()
        .map(|&c| if peak > 0. { (c / peak) as f32 } else { 0. })
        .collect();
    (VERTICAL, horizontal, peak, lumens, normalised)
}

impl Importer for IesImporter {
    fn id(&self) -> &str {
        "ara.ies"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn register_settings(&self, _: &mut ara_schema::SchemaDb) {}
    fn import(
        &self,
        input: &ImportInput,
        _: &mut ImportContext<'_>,
    ) -> ara_asset::Result<ImportOutput> {
        let text = String::from_utf8_lossy(&input.source);
        let photometry = Photometry::parse(&text)?;
        let (vertical, horizontal, peak, lumens, grid) = resample(&photometry);
        if peak.is_nan() || peak <= 0. {
            return Err(Error::Invalid(
                "IES profile: every candela value is zero".into(),
            ));
        }
        let mut bytes = b"ARAIES01".to_vec();
        for word in [vertical as u32, horizontal as u32] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for value in [peak as f32, lumens as f32].into_iter().chain(grid) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "profile.bin".into(),
                kind: ArtifactKind::Custom,
                bytes,
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A symmetric downlight: 1000 cd at nadir falling to zero at 90 degrees.
    const DOWNLIGHT: &str = "IESNA:LM-63-2002\n[TEST] fixture\n[MANUFAC] Ara\nTILT=NONE\n1 -1 2.0 3 1 1 2 0.1 0.1 0\n1.0 1.0 50\n0 45 90\n0\n500 250 0\n";

    #[test]
    fn parses_and_resamples_a_symmetric_downlight() {
        let photometry = Photometry::parse(DOWNLIGHT).expect("valid LM-63");
        assert_eq!(photometry.candela[0], vec![1000., 500., 0.]);
        assert!((photometry.candela(22.5, 123.) - 750.).abs() < 1e-9);
        assert_eq!(
            photometry.candela(120., 0.),
            0.,
            "nothing above the horizon"
        );
        let (vertical, horizontal, peak, lumens, grid) = resample(&photometry);
        assert_eq!((vertical, horizontal, peak), (VERTICAL, 1, 1000.));
        assert_eq!(grid[0], 1.);
        assert_eq!(grid[VERTICAL - 1], 0.);
        // 2 pi int_0^{pi/2} I(theta) sin theta: I = 1000 (1 - 2 theta / pi) piecewise.
        let mut expected = 0.;
        for i in 0..100_000 {
            let theta = (i as f64 + 0.5) / 100_000. * PI / 2.;
            expected +=
                photometry.candela(theta.to_degrees(), 0.) * theta.sin() * (PI / 2. / 100_000.);
        }
        assert!(
            (lumens - 2. * PI * expected).abs() / lumens < 1e-3,
            "{lumens}"
        );
    }

    #[test]
    fn folds_quadrant_symmetry_and_rejects_other_photometry() {
        let quadrant = "TILT=NONE\n1 -1 1 2 2 1 2 0 0 0\n1 1 0\n0 90\n0 90\n100 0\n300 0\n";
        let p = Photometry::parse(quadrant).expect("quadrant profile");
        assert_eq!(p.candela(0., 270.), 300., "270 folds onto 90");
        assert_eq!(p.candela(0., 180.), 100., "180 folds onto 0");
        let type_b = quadrant.replacen(" 2 1 2 ", " 2 2 2 ", 1);
        assert!(Photometry::parse(&type_b).is_err());
        let tilted = "IESNA91\nTILT=INCLUDE\n1\n2\n0 90\n1 0.5\n1 -1 1 2 1 1 2 0 0 0\n1 1 0\n0 90\n0\n10 0\n";
        assert_eq!(
            Photometry::parse(tilted).expect("tilt skipped").candela[0],
            vec![10., 0.]
        );
    }
}
