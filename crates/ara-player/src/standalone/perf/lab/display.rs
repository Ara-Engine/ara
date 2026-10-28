//! Cycles scene-linear EXR through the same exposure, grading and display curve as Ara.
use super::*;
use ara_render_fidelity::{colour, environment::FrameEnvironment};
use std::hash::{Hash, Hasher};

pub(super) struct Transform {
    look: FrameEnvironment,
    scale: f32,
    exposure: f32,
    lut: Option<std::sync::Arc<ara_components::post::ColorLut>>,
}
impl Transform {
    pub fn new(look: FrameEnvironment, frame: &RenderFrame, config: &Value) -> Result<Self> {
        ensure!(
            look.ev100.is_some(),
            "matched-transform scoring requires a fixed/manual exposure"
        );
        let lut = look
            .lut
            .and_then(|i| frame.look.luts.get(i as usize))
            .map(|(_, table)| std::sync::Arc::clone(table));
        let scale = config["reference_radiance_scale"].as_f64().unwrap_or(1.) as f32;
        ensure!(
            scale.is_finite() && scale > 0.,
            "invalid reference radiance scale"
        );
        Ok(Self {
            look,
            scale,
            exposure: look.manual_exposure(frame.environment.exposure),
            lut,
        })
    }
    pub fn report(&self) -> Value {
        json!({"domain":"Cycles scene-linear Rec.709 -> Ara display -> LDR FLIP","radiance_scale":self.scale,"exposure_multiplier":self.exposure,"tone_mapper":format!("{:?}",self.look.tone_mapper),"white_balance":self.look.grading.white_balance,"saturation":self.look.grading.saturation,"contrast":self.look.grading.contrast,"vignette":self.look.grading.vignette,"display":self.look.display,"lut":self.look.lut,"bloom":"not baked into the reference transform"})
    }
    pub fn reference(&self, path: &Path, size: [u32; 2], cache: &Path) -> Result<image::RgbaImage> {
        let metadata = std::fs::metadata(path)?;
        let mut key = std::collections::hash_map::DefaultHasher::new();
        (
            "ara.display-reference/2",
            path,
            size,
            metadata.len(),
            metadata.modified()?,
            self.report().to_string(),
        )
            .hash(&mut key);
        self.look.lut_contribution.to_bits().hash(&mut key);
        if let Some(lut) = &self.lut {
            (lut.size, &lut.texels).hash(&mut key);
        }
        std::fs::create_dir_all(cache)?;
        let cached = cache.join(format!("{:016x}.png", key.finish()));
        if let Ok(image) = image::open(&cached)
            && image.width() == size[0]
            && image.height() == size[1]
        {
            return Ok(image.into_rgba8());
        }
        let input = image::open(path)?.into_rgb32f();
        let input = if input.dimensions() != (size[0], size[1]) {
            image::imageops::resize(
                &input,
                size[0],
                size[1],
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            input
        };
        let mut output = image::RgbaImage::new(size[0], size[1]);
        for (x, y, pixel) in input.enumerate_pixels() {
            ensure!(
                pixel.0.iter().all(|v| v.is_finite()),
                "reference contains non-finite radiance"
            );
            let rgb = pixel.0.map(|v| v.max(0.) * self.scale * self.exposure);
            let uv = [
                (x as f32 + 0.5) / size[0] as f32,
                (y as f32 + 0.5) / size[1] as f32,
            ];
            let d = [(uv[0] - 0.5) * size[0] as f32 / size[1] as f32, uv[1] - 0.5];
            let falloff =
                1. / (1. + (d[0] * d[0] + d[1] * d[1]) * self.look.grading.vignette * 2.5);
            let graded = self.look.grading.apply(rgb.into()) * (falloff * falloff);
            let mut display = colour::tone_map(graded, self.look.tone_mapper).to_array();
            for (i, value) in display.iter_mut().enumerate() {
                *value = (self.look.display[2][i]
                    * (*value + self.look.display[0][i] * (1. - *value)))
                    .clamp(0., 1.)
                    .powf(1. / self.look.display[1][i].max(1e-3));
            }
            let mut srgb = display.map(|v| {
                let v = v.max(0.);
                if v <= 0.0031308 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1. / 2.4) - 0.055
                }
            });
            if let Some(lut) = &self.lut {
                srgb = sample_lut(lut, srgb, self.look.lut_contribution);
            }
            let srgb = srgb.map(|v| (v.clamp(0., 1.) * 255.).round() as u8);
            output.put_pixel(x, y, image::Rgba([srgb[0], srgb[1], srgb[2], 255]));
        }
        let temporary = cached.with_extension(format!("{}.tmp", std::process::id()));
        output.save_with_format(&temporary, image::ImageFormat::Png)?;
        std::fs::rename(temporary, cached)?;
        Ok(output)
    }
}
fn sample_lut(lut: &ara_components::post::ColorLut, c: [f32; 3], weight: f32) -> [f32; 3] {
    let scale = (lut.size - 1) as f32;
    let p = c.map(|v| v.clamp(0., 1.) * scale);
    let low = p.map(|v| v.floor() as u32);
    let high = low.map(|v| (v + 1).min(lut.size - 1));
    let f = std::array::from_fn::<_, 3, _>(|i| p[i] - low[i] as f32);
    let mut sum = [0.; 3];
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let index = [
                    if x == 0 { low[0] } else { high[0] },
                    if y == 0 { low[1] } else { high[1] },
                    if z == 0 { low[2] } else { high[2] },
                ];
                let texel = lut.texels
                    [(index[0] + lut.size * (index[2] + lut.size * index[1])) as usize]
                    .to_le_bytes();
                let w = if x == 0 { 1. - f[0] } else { f[0] }
                    * if y == 0 { 1. - f[1] } else { f[1] }
                    * if z == 0 { 1. - f[2] } else { f[2] };
                for i in 0..3 {
                    sum[i] += w * f32::from(texel[i]) / 255.;
                }
            }
        }
    }
    std::array::from_fn(|i| c[i] + weight * (sum[i] - c[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_linear_reference_uses_the_ara_curve_and_invalidates_cached_exposure() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-player/display-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root)?;
        let input = root.join("reference.exr");
        image::Rgb32FImage::from_pixel(1, 1, image::Rgb([0.18; 3])).save(&input)?;
        let look = FrameEnvironment::resolve(&Default::default(), Default::default(), 1.);
        let mut transform = Transform {
            look,
            scale: 1.,
            exposure: 1.,
            lut: None,
        };
        let first = transform.reference(&input, [1, 1], &root.join("cache"))?;
        let mapped = colour::tone_map([0.18; 3].into(), look.tone_mapper)[0];
        let expected = ((1.055 * mapped.powf(1. / 2.4) - 0.055) * 255.).round() as u8;
        assert_eq!(first.get_pixel(0, 0).0, [expected, expected, expected, 255]);
        assert_eq!(
            first,
            transform.reference(&input, [1, 1], &root.join("cache"))?
        );
        transform.exposure = 2.;
        assert!(
            transform
                .reference(&input, [1, 1], &root.join("cache"))?
                .get_pixel(0, 0)[0]
                > expected
        );
        assert_eq!(std::fs::read_dir(root.join("cache"))?.count(), 2);
        Ok(())
    }
}
