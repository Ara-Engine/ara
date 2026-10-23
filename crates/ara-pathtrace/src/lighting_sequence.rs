//! Lighting-response measurements with fixed exposure and independent steady states.
use super::*;
use ara_gpu::cooked::write_json;

fn mean(data: &[[f32; 4]]) -> f64 {
    corridor::roi(data)
        .iter()
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum::<f64>()
        / (260. * 115.)
}
fn display_luminance(image: &image::RgbaImage) -> Vec<f64> {
    (235..350)
        .flat_map(|y| {
            (170..430).map(move |x| {
                let p = image.get_pixel(x, y);
                let linear = |v: u8| {
                    let v = f64::from(v) / 255.;
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                };
                0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2])
            })
        })
        .collect()
}
pub(crate) fn run(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let mut scene = content::load(&root.join(".sn/assets/sponza"))?;
    scene.lights.push(ara_render::scene::Light {
        position_range: [-1., 2.4, 0., 20.],
        color_lumens: [1., 0.62, 0.3, 0.],
        direction_outer: [0., -1., 0., -1.],
        parameters: [0., 1., 0., 0.],
    });
    let camera = content::camera();
    let mut s = settings([640, 360], [640, 360]);
    s.post = large::fixed_post();
    let mut view = FidelityView::new(gpu, &scene, s, None)?;
    view.set_gather_events(1)?;
    view.set_transport_stride(4)?;
    let mut tick = 0;
    let mut render =
        |view: &mut FidelityView, gpu: &mut VulkanDevice, changed: bool| -> Result<()> {
            let mut f = input(&camera, tick, false);
            f.deterministic = false;
            f.lighting_changed = changed;
            view.render(gpu, &f)?;
            tick += 1;
            Ok(())
        };
    for _ in 0..256 {
        render(&mut view, gpu, false)?;
    }
    let mut strip = image::RgbaImage::new(640 * 8, 360 * 3);
    let mut events = Vec::new();
    for (name, lit, angle) in [
        ("sun-sweep", false, 0.6_f32),
        ("light-on", true, 0.6),
        ("light-off", false, 0.6),
    ] {
        let mut samples = Vec::new();
        let mut direct = Vec::new();
        let mut incident = Vec::new();
        let mut probe_counts = Vec::new();
        let mut previous = Vec::<[f32; 4]>::new();
        let mut spatial_deltas = Vec::new();
        let mut display_previous = Vec::<f64>::new();
        let mut display_delta = 0.;
        let mut display_energy = 0.;
        for frame in 0..192 {
            let changing = frame == 0 || (name == "sun-sweep" && frame < 16);
            if changing {
                let t = if name == "sun-sweep" {
                    (frame + 1).min(16) as f32 / 16.
                } else {
                    1.
                };
                scene.sun_direction = glam::Quat::from_rotation_y(angle * t)
                    * glam::Vec3::new(-0.3, 0.8, 0.45).normalize();
                scene.lights[0].color_lumens[3] = if lit { 12000. } else { 0. };
                view.update_scene(gpu, &scene)?;
            }
            render(&mut view, gpu, changing)?;
            let data = view.capture_radiance(gpu)?;
            samples.push(mean(&data));
            direct.push(mean(&view.capture_direct_radiance(gpu)?));
            incident.push(mean(&view.capture_incident_radiance(gpu)?));
            if frame < 32 {
                let bake = view.bake_snapshot(gpu)?;
                probe_counts.push(
                    bake.data
                        .as_chunks::<102>()
                        .0
                        .iter()
                        .filter(|p| p[100][3] > 0.5)
                        .count(),
                );
            }
            if frame >= 161 {
                spatial_deltas.push(
                    corridor::roi(&data)
                        .iter()
                        .zip(&previous)
                        .map(|(a, b)| {
                            let d = 0.2126 * f64::from(a[0] - b[0])
                                + 0.7152 * f64::from(a[1] - b[1])
                                + 0.0722 * f64::from(a[2] - b[2]);
                            d * d
                        })
                        .sum::<f64>()
                        / (260. * 115.),
                );
            }
            previous = corridor::roi(&data);
            if frame >= 160 {
                let l = display_luminance(&view.capture(gpu)?);
                if !display_previous.is_empty() {
                    display_delta += l
                        .iter()
                        .zip(&display_previous)
                        .map(|(a, b)| (a - b).powi(2))
                        .sum::<f64>();
                    display_energy += l.iter().map(|v| v * v).sum::<f64>();
                }
                display_previous = l;
            }
            if [0, 3, 7, 31, 160, 168, 176, 184].contains(&frame) {
                let column = [0, 3, 7, 31, 160, 168, 176, 184]
                    .iter()
                    .position(|n| *n == frame)
                    .expect("Capture frame");
                let row = events.len();
                image::imageops::replace(
                    &mut strip,
                    &view.capture(gpu)?,
                    (column * 640) as i64,
                    (row * 360) as i64,
                );
            }
        }
        let steady = samples[160..].iter().sum::<f64>() / 32.;
        let indirect = samples
            .iter()
            .zip(&direct)
            .map(|(total, direct)| total - direct)
            .collect::<Vec<_>>();
        let steady_indirect = indirect[160..].iter().sum::<f64>() / 32.;
        let end = if name == "sun-sweep" { 15 } else { 0 };
        let convergence = (end..160)
            .find(|i| {
                indirect[*i..*i + 8]
                    .iter()
                    .all(|v| (v / steady_indirect - 1.).abs() <= 0.05)
            })
            .map(|i| i - end + 1);
        let differences = samples[161..]
            .iter()
            .zip(&samples[160..191])
            .map(|(a, b)| (a - b) / steady)
            .collect::<Vec<_>>();
        let mean_delta = differences.iter().sum::<f64>() / differences.len() as f64;
        let mean_square = differences.iter().map(|v| v * v).sum::<f64>() / differences.len() as f64;
        let flicker = (mean_square - mean_delta * mean_delta).max(0.);
        let display_rms = (display_delta / display_energy.max(1e-12)).sqrt();
        println!(
            "{name}: convergence {convergence:?} frames; normalized luminance delta variance {flicker:.8}"
        );
        anyhow::ensure!(
            display_rms <= 0.01,
            "{name}: display flicker RMS exceeds 1%"
        );
        events.push(serde_json::json!({"event":name,"convergence_frames":convergence,"flicker_variance":flicker,"flicker_rms":mean_square.sqrt(),"display_pixel_flicker_rms":display_rms,"pixel_flicker_variance":spatial_deltas.iter().sum::<f64>()/spatial_deltas.len() as f64/(steady*steady),"steady_luminance":steady,"steady_indirect_luminance":steady_indirect,"luminance":samples,"direct_luminance":direct,"incident_luminance":incident,"active_probes":probe_counts}));
    }
    strip.save(output_path(root, "captures/sponza-lighting-sequence.png")?)?;
    view.destroy(gpu)?;
    write_json(
        &output_path(root, "measurements/lighting-stability.json")?,
        &serde_json::json!({"fps":60,"roi":[170,235,430,350],"metric":"filtered indirect HDR (direct subtracted): 8 consecutive samples within 5% of 32-frame settled indirect mean; static final-frame luminance-delta variance; display RMS in decoded linear sRGB","events":events}),
    )?;
    anyhow::ensure!(
        events
            .iter()
            .all(|e| e["convergence_frames"].as_u64().is_some_and(|n| n <= 8)),
        "Lighting response exceeds eight frames"
    );
    gpu.assert_clean()?;
    Ok(())
}
