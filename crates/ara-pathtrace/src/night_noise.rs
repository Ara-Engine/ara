//! Static low-light noise measured before exposure, with matching display strips.
use super::*;
use ara_gpu::cooked::write_json;
const SIZE: [u32; 2] = [480, 270];
fn blocks(data: &[[f32; 4]]) -> Vec<f64> {
    // Sixteen-pixel blocks expose low-frequency boiling in the lower-half ROI.
    let mut out = Vec::new();
    for by in (144..256).step_by(16) {
        for bx in (0..480).step_by(16) {
            let mut sum = 0.;
            for y in by..by + 16 {
                for x in bx..bx + 16 {
                    let c = data[y * 480 + x];
                    sum += 0.2126 * f64::from(c[0])
                        + 0.7152 * f64::from(c[1])
                        + 0.0722 * f64::from(c[2]);
                }
            }
            out.push(sum / 256.);
        }
    }
    out
}
/// Measure an independent-sampling ablation and reservoir reuse with the same rig.
pub fn run(gpu: &mut VulkanDevice, root: &Path, many: bool) -> Result<()> {
    let scene = if many {
        ara_pathtrace::many_lights::scene()
    } else {
        ara_pathtrace::robust_scenes::night()
    };
    let camera = if many {
        ara_pathtrace::many_lights::camera()
    } else {
        ara_pathtrace::robust_scenes::camera(false)
    };
    let name = if many {
        "many-lights-noise"
    } else {
        "night-noise"
    };
    let mut strip = image::RgbaImage::new(480 * 8, 270 * 2);
    let mut report = Vec::new();
    for (row, reservoirs) in [false, true].into_iter().enumerate() {
        let mut s = settings(SIZE, SIZE);
        s.post = PostSettings {
            ev100: Some(if many { 6. } else { 5. }),
            ..large::fixed_post()
        };
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        view.set_gather_events(2)?;
        view.set_transport_stride(2)?;
        view.set_reservoir_sampling(gpu, reservoirs)?;
        let mut old = Vec::<f64>::new();
        let mut delta = 0.;
        let mut energy = 0.;
        let mut pixel_delta = 0.;
        let mut pixel_energy = 0.;
        let mut previous = Vec::<[f32; 4]>::new();
        for tick in 0..320 {
            let mut f = input(&camera, tick, false);
            f.deterministic = false;
            view.render(gpu, &f)?;
            if tick < 256 {
                continue;
            }
            let hdr = view.capture_radiance(gpu)?;
            let value = blocks(&hdr);
            if !old.is_empty() {
                delta += value
                    .iter()
                    .zip(&old)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>();
                energy += value.iter().map(|x| x * x).sum::<f64>();
                for (a, b) in hdr[(480 * 135)..].iter().zip(&previous[(480 * 135)..]) {
                    let x = 0.2126 * f64::from(a[0])
                        + 0.7152 * f64::from(a[1])
                        + 0.0722 * f64::from(a[2]);
                    let y = 0.2126 * f64::from(b[0])
                        + 0.7152 * f64::from(b[1])
                        + 0.0722 * f64::from(b[2]);
                    pixel_delta += (x - y).powi(2);
                    pixel_energy += x * x;
                }
            }
            old = value;
            previous = hdr;
            if (tick - 256) % 8 == 0 {
                image::imageops::replace(
                    &mut strip,
                    &view.capture(gpu)?,
                    i64::from(((tick - 256) / 8) * 480),
                    (row * 270) as i64,
                );
            }
        }
        let block_rms = (delta / energy.max(1e-20)).sqrt();
        println!("Night reservoirs={reservoirs}: low-frequency temporal RMS {block_rms:.6}");
        report.push(serde_json::json!({"reservoirs":reservoirs,"size":SIZE,"warmup":256,"frames":64,"block_size":16,"roi":[0,144,480,256],"low_frequency_luminance_delta_rms":block_rms,"pixel_luminance_delta_rms":(pixel_delta/pixel_energy.max(1e-20)).sqrt(),"scene_hash":view.scene_hash(),"shader_revision":view.shader_revision()}));
        view.destroy(gpu)?;
    }
    strip.save(root.join(format!("captures/{name}-strip.png")))?;
    write_json(&root.join(format!("measurements/{name}.json")), &report)?;
    gpu.assert_clean()?;
    Ok(())
}
