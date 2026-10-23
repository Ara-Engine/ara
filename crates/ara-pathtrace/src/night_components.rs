//! Isolate direct-light bias from path-reconnection bias in the night regression.
use super::*;
use ara_gpu::cooked::write_json;

pub fn run(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let size = [480, 270];
    let scene = ara_pathtrace::robust_scenes::night();
    let camera = ara_pathtrace::robust_scenes::camera(false);
    let mut s = settings(size, size);
    s.post = PostSettings {
        ev100: Some(5.),
        ..large::fixed_post()
    };
    let mut view = FidelityView::new(gpu, &scene, s, None)?;
    ara_pathtrace::robust_scenes::configure_view(gpu, &mut view, false)?;
    for tick in 0..256 {
        let mut f = input(&camera, tick, false);
        f.deterministic = false;
        view.render(gpu, &f)?;
    }
    let hybrid = view.capture_radiance(gpu)?;
    large::save_hdr(&root.join(".sn/night-trial-hybrid.f32"), &hybrid)?;
    let direct = view.capture_direct_radiance(gpu)?;
    view.destroy(gpu)?;
    let mut reference = Reference::new(gpu, &scene, size, 4096, 1, s.post)?;
    reference.converge(gpu, &camera)?;
    let ground_direct = reference.radiance(gpu)?;
    reference.destroy(gpu)?;
    large::save_hdr(&root.join(".sn/night-direct-reference.f32"), &ground_direct)?;
    large::save_hdr(&root.join(".sn/night-direct-hybrid.f32"), &direct)?;
    let bytes = std::fs::read(root.join(".sn/night_interior-reference.f32"))?;
    let ground = bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|p| {
            std::array::from_fn::<_, 4, _>(|i| {
                f32::from_le_bytes([p[4 * i], p[4 * i + 1], p[4 * i + 2], p[4 * i + 3]])
            })
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(ground.len() == hybrid.len(), "Reference dimensions differ");
    let lower = hybrid.len() / 2;
    let error = ara_pathtrace::metrics::radiance(&hybrid[lower..], &ground[lower..])?;
    println!(
        "Night trial lower-half relative RMSE: {:.6}",
        error.relative_rmse
    );
    let indirect = hybrid
        .iter()
        .zip(&direct)
        .map(|(h, d)| std::array::from_fn(|i| h[i] - d[i]))
        .collect::<Vec<_>>();
    let ground_indirect = ground
        .iter()
        .zip(&ground_direct)
        .map(|(h, d)| std::array::from_fn(|i| h[i] - d[i]))
        .collect::<Vec<_>>();
    let mut report = Vec::new();
    for (name, bounds) in [
        ("floor", [30, 230, 450, 260]),
        ("wall", [360, 80, 440, 160]),
        ("lower_half", [0, 135, 480, 270]),
    ] {
        let mut row = serde_json::json!({"roi":name,"bounds":bounds});
        for (key, a, b) in [
            ("direct", &direct, &ground_direct),
            ("indirect", &indirect, &ground_indirect),
        ] {
            let mut ah = Vec::new();
            let mut bh = Vec::new();
            for y in bounds[1]..bounds[3] {
                for x in bounds[0]..bounds[2] {
                    ah.push(a[y * 480 + x]);
                    bh.push(b[y * 480 + x]);
                }
            }
            let mean = |p: &Vec<[f32; 4]>| {
                p.iter()
                    .map(|v| {
                        0.2126 * f64::from(v[0])
                            + 0.7152 * f64::from(v[1])
                            + 0.0722 * f64::from(v[2])
                    })
                    .sum::<f64>()
                    / p.len() as f64
            };
            row[key] = serde_json::json!({"error":ara_pathtrace::metrics::radiance(&ah,&bh)?,"mean_ratio":mean(&ah)/mean(&bh)});
        }
        report.push(row);
    }
    write_json(&root.join(".sn/night-components.json"), &report)?;
    println!("Night direct/indirect separation: {report:?}");
    gpu.assert_clean()?;
    Ok(())
}
