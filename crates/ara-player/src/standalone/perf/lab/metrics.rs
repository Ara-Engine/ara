//! Quality regions share the benchmark's crop-before-FLIP convention.
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use serde_json::{Value, json};
use std::path::Path;

fn rect(value: &Value, size: [u32; 2]) -> Result<[u32; 4]> {
    let rect: [f64; 4] = serde_json::from_value(value.clone())?;
    ensure!(
        rect.iter().all(|v| v.is_finite() && (0. ..=1.).contains(v))
            && rect[0] < rect[2]
            && rect[1] < rect[3],
        "invalid normalized region"
    );
    let pixels = std::array::from_fn(|i| (rect[i] * f64::from(size[i % 2])).round() as u32);
    ensure!(
        pixels[0] < pixels[2] && pixels[1] < pixels[3],
        "region smaller than one capture pixel"
    );
    Ok(pixels)
}
pub(super) fn regions(
    config: &Value,
    shot: &str,
    image: &RgbaImage,
    reference: &RgbaImage,
    output: &Path,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for region in config["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["shot"] == shot)
    {
        let name = region["name"].as_str().context("region name")?;
        ensure!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "invalid region name"
        );
        let [x0, y0, x1, y1] = rect(&region["rect"], [image.width(), image.height()])?;
        let crop = |image: &RgbaImage| {
            image::imageops::crop_imm(image, x0, y0, x1 - x0, y1 - y0).to_image()
        };
        let actual = crop(image);
        let truth = crop(reference);
        let flip =
            ara_pathtrace::flip::flip(&truth, &actual, ara_pathtrace::flip::PIXELS_PER_DEGREE)?;
        let ssim = 1. - ara_render::capture::compare(&actual, &truth, 2.)?.score;
        let png = output.join(format!("{shot}-{name}.png"));
        let reference_png = output.join(format!("{shot}-{name}-reference.png"));
        actual.save(&png)?;
        truth.save(&reference_png)?;
        result.push(json!({"name":name,"rect_pixels":[x0,y0,x1,y1],"png":png,"reference_png":reference_png,"ldr_secondary":{"flip":flip.mean,"ssim":ssim},"gate":{"status":"not_measured","reason":"matched-transform reference and calibrated baseline required","threshold":region["threshold"]},"legacy_ldr":region["legacy_ldr"]}));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalized_crops_recover_original_pixels_and_scale() -> Result<()> {
        let value = json!([2050. / 3840., 1010. / 2160., 3570. / 3840., 1610. / 2160.]);
        assert_eq!(rect(&value, [3840, 2160])?, [2050, 1010, 3570, 1610]);
        assert_eq!(
            rect(&json!([0., 0., 1., 1.]), [640, 360])?,
            [0, 0, 640, 360]
        );
        assert!(rect(&json!([0.5, 0., 0.25, 1.]), [640, 360]).is_err());
        Ok(())
    }
}
