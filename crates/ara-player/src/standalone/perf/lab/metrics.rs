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
    reference: Option<&RgbaImage>,
    matched: Option<&RgbaImage>,
    repeats: &[RgbaImage],
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
        let prepared_actual = if reference.is_some() || matched.is_some() {
            Some(ara_pathtrace::flip::PreparedImage::new(
                &actual,
                ara_pathtrace::flip::PIXELS_PER_DEGREE,
            )?)
        } else {
            None
        };
        let png = output.join(format!("{shot}-{name}.png"));
        actual.save(&png)?;
        let mut value = json!({"name":name,"rect_pixels":[x0,y0,x1,y1],"png":png,"gate":{"status":"not_measured","reason":"matched-transform reference required","threshold":region["threshold"]},"legacy_ldr":region["legacy_ldr"]});
        if let Some(reference) = reference {
            let truth = crop(reference);
            let flip = ara_pathtrace::flip::PreparedImage::new(
                &truth,
                ara_pathtrace::flip::PIXELS_PER_DEGREE,
            )?
            .compare(prepared_actual.as_ref().context("prepared crop")?)?;
            let ssim = 1. - ara_render::capture::compare(&actual, &truth, 2.)?.score;
            let reference_png = output.join(format!("{shot}-{name}-reference.png"));
            truth.save(&reference_png)?;
            value["reference_png"] = json!(reference_png);
            value["ldr_secondary"] = json!({"flip":flip.mean,"ssim":ssim});
        }
        if let Some(reference) = matched {
            let truth = crop(reference);
            let prepared_truth = ara_pathtrace::flip::PreparedImage::new(
                &truth,
                ara_pathtrace::flip::PIXELS_PER_DEGREE,
            )?;
            let mut scores = vec![
                prepared_truth
                    .compare(prepared_actual.as_ref().context("prepared crop")?)?
                    .mean,
            ];
            for image in repeats {
                scores.push(
                    prepared_truth
                        .compare(&ara_pathtrace::flip::PreparedImage::new(
                            &crop(image),
                            ara_pathtrace::flip::PIXELS_PER_DEGREE,
                        )?)?
                        .mean,
                );
            }
            let mean = scores.iter().sum::<f64>() / scores.len() as f64;
            let deviation = scores
                .iter()
                .map(|value| (value - mean).abs())
                .fold(0_f64, f64::max);
            let tolerance = config["repeatability"]["max_deviation"]
                .as_f64()
                .unwrap_or(0.002);
            let repeatable = scores.len() >= 3 && deviation <= tolerance;
            let max = region["threshold"]["matched_max"].as_f64().or_else(|| {
                let profile = config["capture_profile"].as_str().unwrap_or("");
                let baseline = if region["threshold"]["baseline_by_profile"].is_object() {
                    &region["threshold"]["baseline_by_profile"][profile]
                } else {
                    &region["threshold"]["baseline"]
                };
                baseline
                    .as_f64()
                    .map(|b| b + region["threshold"]["tolerance"].as_f64().unwrap_or(0.002))
            });
            let matched_png = output.join(format!("{shot}-{name}-matched-reference.png"));
            truth.save(&matched_png)?;
            value["matched_transform"] = json!({"flip":mean,"run_scores":scores,"reference_png":matched_png,"repeatability":{"runs":scores.len(),"max_deviation":deviation,"tolerance":tolerance,"status":if scores.len()<3{"not_measured"}else if repeatable{"pass"}else{"fail"}}});
            value["gate"] = json!({"status":if scores.len()>=3&&!repeatable || max.is_some_and(|limit|mean>limit){"fail"}else if max.is_none(){"uncalibrated"}else if repeatable{"pass"}else{"not_measured"},"capture_profile":config["capture_profile"],"matched_max":max,"repeatability_required":3,"legacy_ldr_is_a_gate":false});
        }
        result.push(value);
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

    #[test]
    fn only_matched_three_run_scores_can_pass_and_unstable_runs_fail() -> Result<()> {
        let output = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-player/region-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
        std::fs::create_dir_all(&output)?;
        let config = json!({"regions":[{"name":"metal","shot":"materials","rect":[0.,0.,1.,1.],"threshold":{"matched_max":0.01},"legacy_ldr":{"ara":0.9}}],"repeatability":{"max_deviation":0.002}});
        let black = RgbaImage::from_pixel(32, 16, image::Rgba([0, 0, 0, 255]));
        let white = RgbaImage::from_pixel(32, 16, image::Rgba([255, 255, 255, 255]));
        let score = |matched: Option<&RgbaImage>, runs: &[RgbaImage]| {
            regions(
                &config,
                "materials",
                &black,
                Some(&white),
                matched,
                runs,
                &output,
            )
        };
        assert_eq!(score(None, &[])?[0]["gate"]["status"], "not_measured");
        assert_eq!(
            score(Some(&black), &[])?[0]["gate"]["status"],
            "not_measured"
        );
        assert_eq!(
            score(Some(&black), &[black.clone(), black.clone()])?[0]["gate"]["status"],
            "pass"
        );
        let unstable = score(Some(&black), &[black.clone(), white.clone()])?;
        assert_eq!(unstable[0]["gate"]["status"], "fail");
        assert_eq!(
            unstable[0]["matched_transform"]["repeatability"]["status"],
            "fail"
        );
        Ok(())
    }
}
