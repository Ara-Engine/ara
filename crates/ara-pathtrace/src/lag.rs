//! Probe response to a four-stop lighting step, independent of direct-light pixels.
use super::*;
use ara_gpu::cooked::write_json;
use serde::Serialize;

#[derive(Serialize)]
struct Response {
    old_irradiance: f64,
    target_irradiance: f64,
    measured_irradiance: Vec<f64>,
    normalized_response: Vec<f64>,
    frames_to_90_percent: Option<usize>,
}
fn irradiance(view: &mut FidelityView, gpu: &mut VulkanDevice) -> Result<f64> {
    let snapshot = view.bake_snapshot(gpu)?;
    let mut sum = 0.;
    let mut count = 0;
    for probe in snapshot
        .data
        .as_chunks::<{ ara_render_fidelity::probes::PROBE_VECTORS }>()
        .0
    {
        if probe[100][3] < 0.5 {
            continue;
        }
        for texel in &probe[..36] {
            sum += f64::from(texel[0]) * 0.2126
                + f64::from(texel[1]) * 0.7152
                + f64::from(texel[2]) * 0.0722;
            count += 1;
        }
    }
    anyhow::ensure!(count > 0, "No classified probes in lag test");
    Ok(sum / f64::from(count))
}
pub fn capture(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let mut scene = scenes::scene("glossy_interior", 0);
    let camera = scenes::camera();
    let mut view = FidelityView::new(gpu, &scene, settings([160, 90], [160, 90]), None)?;
    for tick in 0..96 {
        view.render(gpu, &input(&camera, tick, false))?;
    }
    let old = irradiance(&mut view, gpu)?;
    scene.lights[0].color_lumens[3] *= 16.;
    view.update_scene(gpu, &scene)?;
    let mut measured = Vec::new();
    for tick in 0..48 {
        let mut request = input(&camera, 100 + tick, false);
        request.lighting_changed = tick == 0;
        view.render(gpu, &request)?;
        if tick < 24 {
            measured.push(irradiance(&mut view, gpu)?);
        }
    }
    let target = irradiance(&mut view, gpu)?;
    anyhow::ensure!(
        target > old * 2.,
        "Light step did not reach the probe field"
    );
    let normalized = measured
        .iter()
        .map(|v| (v - old) / (target - old))
        .collect::<Vec<_>>();
    let reached = normalized.iter().position(|v| *v >= 0.9).map(|i| i + 1);
    println!("DDGI four-stop response: 90% in {reached:?} frames");
    write_json(
        &root.join("measurements/lighting-step-response.json"),
        &Response {
            old_irradiance: old,
            target_irradiance: target,
            measured_irradiance: measured,
            normalized_response: normalized,
            frames_to_90_percent: reached,
        },
    )?;
    anyhow::ensure!(
        reached.is_some_and(|v| v <= 12),
        "DDGI lighting lag exceeds twelve frames"
    );
    view.destroy(gpu)?;
    Ok(())
}
