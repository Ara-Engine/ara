//! The Night-Noise block-luminance RMS, generalized to the active preset and camera.
use super::*;

struct Accumulator {
    previous: Vec<f64>,
    delta: Vec<f64>,
    energy: Vec<f64>,
    pairs: u32,
}
impl Accumulator {
    fn add(&mut self, values: Vec<f64>) {
        if self.previous.is_empty() {
            self.delta = vec![0.; values.len()];
            self.energy = vec![0.; values.len()];
        } else {
            for (((delta, energy), current), previous) in self
                .delta
                .iter_mut()
                .zip(&mut self.energy)
                .zip(&values)
                .zip(&self.previous)
            {
                *delta += (current - previous).powi(2);
                *energy += current * current;
            }
            self.pairs += 1;
        }
        self.previous = values;
    }
}
fn blocks(pixels: &[[f32; 4]], size: [u32; 2]) -> Result<Vec<f64>> {
    ensure!(
        pixels.len() == size[0] as usize * size[1] as usize,
        "temporal radiance extent mismatch"
    );
    ensure!(
        pixels.iter().flatten().all(|v| v.is_finite()),
        "non-finite temporal radiance"
    );
    let mut result = Vec::new();
    for y0 in (0..size[1]).step_by(16) {
        for x0 in (0..size[0]).step_by(16) {
            let mut sum = 0.;
            let mut count = 0.;
            for y in y0..(y0 + 16).min(size[1]) {
                for x in x0..(x0 + 16).min(size[0]) {
                    let p = pixels[(y * size[0] + x) as usize];
                    sum += 0.2126 * f64::from(p[0])
                        + 0.7152 * f64::from(p[1])
                        + 0.0722 * f64::from(p[2]);
                    count += 1.;
                }
            }
            result.push(sum / count);
        }
    }
    Ok(result)
}
/// Preserve live accumulation and simulation; the pan advances only the view transform.
pub(super) fn run(
    presenter: &mut Presenter,
    frame: &mut RenderFrame,
    directory: &Path,
    shot: &str,
    config: &Value,
) -> Result<Value> {
    let frames = config["frames"].as_u64().unwrap_or(8);
    ensure!(
        (3..=128).contains(&frames),
        "temporal sample count must be 3..128"
    );
    let camera = frame.camera.context("temporal camera is not set")?;
    let speed = config["pan_degrees_per_second"].as_f64().unwrap_or(2.);
    ensure!(
        speed.is_finite() && (0. ..=30.).contains(&speed),
        "invalid pan speed"
    );
    std::fs::create_dir_all(directory)?;
    let result = (|| -> Result<Value> {
        let mut report = json!({"format":"ara.lab.temporal/1","domain":"scene-linear pre-exposure lighting","block_size":16,"frames":frames,"pan_degrees_per_second":speed,"sample_interval_seconds":1./60.,"simulation":"frozen","definition":"sqrt(sum successive block-luminance deltas squared / sum block luminance squared), matching Night-Noise"});
        for mode in ["static", "pan"] {
            frame.camera = Some(camera);
            let mut accumulator = Accumulator {
                previous: Vec::new(),
                delta: Vec::new(),
                energy: Vec::new(),
                pairs: 0,
            };
            let mut extent = [0; 2];
            let mut previous_display: Option<ara_pathtrace::flip::PreparedImage> = None;
            let mut perceptual = Vec::new();
            let mut flip_sum = 0.;
            let mut flip_peak = 0_f64;
            for sample in 0..frames {
                if mode == "pan" {
                    let angle = (speed * sample as f64 / 60.).to_radians() as f32;
                    let mut view = camera;
                    view.forward = [
                        camera.forward[0] * angle.cos() + camera.forward[2] * angle.sin(),
                        camera.forward[1],
                        -camera.forward[0] * angle.sin() + camera.forward[2] * angle.cos(),
                    ];
                    frame.camera = Some(view);
                }
                presenter.present(frame)?;
                ensure!(
                    !presenter.readiness().failed,
                    "temporal sample retained a failed presentation; inspect player logs"
                );
                let (size, pixels) = presenter.lab_radiance()?;
                ensure!(
                    extent == [0; 2] || extent == size,
                    "temporal render extent changed"
                );
                extent = size;
                accumulator.add(blocks(&pixels, size)?);
                let display = presenter.lab_display(frame)?;
                let prepared = ara_pathtrace::flip::PreparedImage::new(
                    &display,
                    ara_pathtrace::flip::PIXELS_PER_DEGREE,
                )?;
                if let Some(previous) = previous_display.as_ref() {
                    let difference = previous.compare(&prepared)?;
                    let per_block =
                        block_means(&difference.errors, [display.width(), display.height()]);
                    flip_sum += difference.mean;
                    flip_peak = flip_peak.max(per_block.iter().copied().fold(0_f64, f64::max));
                    perceptual.push(
                        json!({"pair":sample,"mean":difference.mean,"block_means":per_block}),
                    );
                }
                previous_display = Some(prepared);
            }
            let rms = accumulator
                .delta
                .iter()
                .zip(&accumulator.energy)
                .map(|(d, e)| (d / e.max(1e-20)).sqrt())
                .collect::<Vec<_>>();
            let global = (accumulator.delta.iter().sum::<f64>()
                / accumulator.energy.iter().sum::<f64>().max(1e-20))
            .sqrt();
            let peak = rms.iter().copied().fold(0_f64, f64::max);
            let mean = rms.iter().sum::<f64>() / rms.len() as f64;
            let path = directory.join(format!("{shot}-temporal-{mode}.json"));
            write(
                &path,
                &json!({"size":extent,"block_size":16,"columns":extent[0].div_ceil(16),"rows":extent[1].div_ceil(16),"relative_luminance_rms":rms,"energy":accumulator.energy,"sample_pairs":accumulator.pairs}),
            )?;
            let mean_limit = config["static_mean_max"].as_f64().unwrap_or(0.01);
            let block_limit = config["static_block_max"].as_f64().unwrap_or(0.03);
            let flip_mean = flip_sum / accumulator.pairs as f64;
            let flip_path = directory.join(format!("{shot}-temporal-{mode}-flip.json"));
            write(
                &flip_path,
                &json!({"metric":"consecutive display-frame LDR FLIP","pixels_per_degree":67,"block_size":16,"pairs":perceptual}),
            )?;
            report[mode] = json!({"size":extent,"low_frequency_luminance_delta_rms":global,"mean_block_rms":mean,"max_block_rms":peak,"blocks":path,"frame_flip":{"mean":flip_mean,"max_block_mean":flip_peak,"pairs":accumulator.pairs,"blocks":flip_path},"gate":if mode=="static" {json!({"metric":"consecutive display-frame FLIP","status":if flip_mean<=mean_limit&&flip_peak<=block_limit{"pass"}else{"fail"},"mean_limit":mean_limit,"block_limit":block_limit})}else{json!({"status":"informational_motion"})}});
        }
        Ok(report)
    })();
    frame.camera = Some(camera);
    result
}

fn block_means(values: &[f32], size: [u32; 2]) -> Vec<f64> {
    let mut means = Vec::new();
    for y0 in (0..size[1]).step_by(16) {
        for x0 in (0..size[0]).step_by(16) {
            let mut sum = 0.;
            let mut count = 0.;
            for y in y0..(y0 + 16).min(size[1]) {
                for x in x0..(x0 + 16).min(size[0]) {
                    sum += f64::from(values[(y * size[0] + x) as usize]);
                    count += 1.;
                }
            }
            means.push(sum / count);
        }
    }
    means
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_blocks_have_zero_noise_and_partial_blocks_keep_their_area() -> Result<()> {
        let pixels = vec![[2., 2., 2., 1.]; 17 * 17];
        let values = blocks(&pixels, [17, 17])?;
        assert_eq!(values.len(), 4);
        assert!(values.iter().all(|v| (*v - 2.).abs() < 1e-12));
        let mut a = Accumulator {
            previous: Vec::new(),
            delta: Vec::new(),
            energy: Vec::new(),
            pairs: 0,
        };
        a.add(values);
        a.add(blocks(&pixels, [17, 17])?);
        assert!(a.delta.iter().all(|v| *v == 0.));
        assert_eq!(a.pairs, 1);
        assert_eq!(block_means(&vec![0.25; 17 * 17], [17, 17]), vec![0.25; 4]);
        Ok(())
    }
}
