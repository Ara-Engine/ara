use anyhow::{Result, ensure};
use ara_geo::{Settings, cook, sample};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::CameraSpec;
use ara_render_geo::shadows::{ShadowReference, VirtualShadows};
use ara_render_geo::{
    DebugView, GeometryConfig, GeometryView, Instance, View, streaming::PagePool,
};
use glam::Vec3;
use std::path::Path;

#[test]
fn lru_pins_roots_and_protects_the_current_feedback_batch() -> Result<()> {
    let mut pool = PagePool::new(3, &[0])?;
    assert_eq!(pool.request(1, [1, 2])?.len(), 2);
    let result = pool.request(2, [2, 3])?;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].evicted, Some(1));
    assert!(pool.slot(0).is_some());
    assert!(pool.slot(2).is_some());
    Ok(())
}

fn draw(
    gpu: &mut VulkanDevice,
    view: &mut GeometryView,
    camera: &CameraSpec,
    hardware: bool,
    full: bool,
) -> Result<Vec<[u32; 2]>> {
    view.update(
        gpu,
        &View {
            camera,
            error_pixels: 0.5,
            reset_history: true,
            debug: DebugView::Lit,
            hardware_only: hardware,
            full_resolution: full,
        },
    )?;
    let list = view.record(gpu)?;
    let p = gpu.submit(list, &[])?;
    view.submitted(p);
    gpu.resolve(p)?;
    view.identities(gpu)
}
fn coverage_bound(a: &[[u32; 2]], b: &[[u32; 2]], size: [usize; 2]) -> Result<()> {
    for (i, id) in a.iter().enumerate() {
        if id[0] == 0 || b[i][0] == id[0] {
            continue;
        }
        let x = (i % size[0]) as i32;
        let y = (i / size[0]) as i32;
        let matches = [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .into_iter()
            .any(|(dx, dy)| {
                let px = x + dx;
                let py = y + dy;
                px >= 0
                    && py >= 0
                    && (px as usize) < size[0]
                    && (py as usize) < size[1]
                    && b[py as usize * size[0] + px as usize][0] == id[0]
            });
        ensure!(
            matches,
            "silhouette/hole differs by more than one pixel at {x},{y}"
        );
    }
    Ok(())
}
#[test]
fn hybrid_mesh_and_indirect_goldens_and_full_resolution_error() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capture = root.join("captures/verification");
    std::fs::create_dir_all(&capture)?;
    let source = sample::rock(96, 192);
    let geometry = cook(&source, Settings::default())?;
    let instances = [
        Instance {
            position_scale: [-1.5, 0.0, 0.0, 1.0],
            color_roughness: [0.4, 0.3, 0.2, 0.7],
            identity: [1, 0, 0, 0],
        },
        Instance {
            position_scale: [1.5, 0.0, 0.0, 1.0],
            color_roughness: [0.2, 0.4, 0.3, 0.7],
            identity: [2, 0, 0, 0],
        },
    ];
    let size = [640, 360];
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0.0, 2.0, 8.0),
        target: Vec3::ZERO,
        vertical_fov: 55.0f32.to_radians(),
    };
    let mut gpu = VulkanDevice::new(&[], capture.join("gpu"))?;
    let mut mesh = GeometryView::new(
        &mut gpu,
        &geometry,
        &instances,
        size,
        10000,
        &root.join("cooked"),
        true,
    )?;
    let hybrid = draw(&mut gpu, &mut mesh, &camera, false, false)?;
    let pixels = mesh.preview_bytes(&mut gpu)?;
    mesh.capture(&mut gpu, &capture.join("hybrid.png"))?;
    ensure!(
        hybrid.iter().filter(|id| id[0] != 0).count() > 1000,
        "empty test scene"
    );
    let stats = mesh.stats(&mut gpu)?;
    ensure!(
        stats.software_triangles > 0 && stats.hardware_triangles > 0 && stats.overflow == 0,
        "both raster backends must execute"
    );
    draw(&mut gpu, &mut mesh, &camera, false, false)?;
    ensure!(
        pixels == mesh.preview_bytes(&mut gpu)?,
        "capture is not byte-deterministic"
    );
    let hardware = draw(&mut gpu, &mut mesh, &camera, true, false)?;
    mesh.capture(&mut gpu, &capture.join("hardware.png"))?;
    coverage_bound(&hybrid, &hardware, [640, 360])?;
    coverage_bound(&hardware, &hybrid, [640, 360])?;
    let full = draw(&mut gpu, &mut mesh, &camera, true, true)?;
    mesh.capture(&mut gpu, &capture.join("full-resolution.png"))?;
    coverage_bound(&hybrid, &full, [640, 360])?;
    coverage_bound(&full, &hybrid, [640, 360])?;
    let golden = root.join("goldens/hybrid.png");
    if std::env::var_os("ARA_UPDATE_GEO_GOLDENS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        std::fs::create_dir_all(root.join("goldens"))?;
        image::save_buffer(&golden, &pixels, 640, 360, image::ColorType::Rgba8)?;
    }
    let expected = image::open(golden)?.to_rgba8();
    ensure!(
        expected.as_raw() == &pixels,
        "geometry golden differs; inspect captures before deliberate update"
    );
    draw(&mut gpu, &mut mesh, &camera, false, false)?;
    let lights = [ara_vsm::directional(
        1,
        Vec3::new(0.6, -1.0, -0.3),
        Vec3::ZERO,
        0,
        8.0,
        1,
    )?];
    let mut shadows = VirtualShadows::new(&mut gpu, &mesh, 2048, 10000, &root.join("cooked"))?;
    for frame in 0..2 {
        shadows.update(&mut gpu, &mesh, &lights, Vec3::ZERO, frame == 0)?;
        let list = shadows.record(&mut gpu, &mesh)?;
        let point = gpu.submit(list, &[])?;
        shadows.submitted(point);
        gpu.resolve(point)?;
        let stats = shadows.stats(&mut gpu, &mesh)?;
        ensure!(
            stats.missing_pages == 0 && stats.cluster_overflow == 0,
            "VSM capacity failure"
        );
        ensure!(
            if frame == 0 {
                stats.dirty_pages > 0
            } else {
                stats.dirty_pages == 0 && stats.cache_hits > 0
            },
            "VSM static cache did not reuse pages"
        );
    }
    shadows.capture(&mut gpu, &mesh, &capture.join("vsm.png"))?;
    let vsm = shadows.samples(&mut gpu, &mesh)?;
    let mut reference =
        ShadowReference::new(&mut gpu, &source, &instances, &mesh, &root.join("cooked"))?;
    let timings = reference.render(&mut gpu, &mesh, Vec3::new(-0.6, 1.0, 0.3))?;
    let rt = reference.samples(&mut gpu, &mesh)?;
    let pixels: Vec<u8> = rt
        .iter()
        .flat_map(|p| {
            let c = (p[0] * 255.0).round() as u8;
            [c, c, c, 255]
        })
        .collect();
    image::save_buffer(
        capture.join("rt-shadow.png"),
        &pixels,
        640,
        360,
        image::ColorType::Rgba8,
    )?;
    let covered = hybrid.iter().filter(|id| id[0] != 0).count();
    let mae = vsm
        .iter()
        .zip(&rt)
        .zip(&hybrid)
        .filter(|(_, id)| id[0] != 0)
        .map(|((v, r), _)| f64::from((v[0] - r[0]).abs()))
        .sum::<f64>()
        / covered as f64;
    ara_gpu::cooked::write_json(
        &capture.join("shadow-comparison.json"),
        &serde_json::json!({"covered_pixels":covered,"visibility_mae":mae,"rt_timings":timings,"note":"PCF versus one hard ray on the same LOD camera surface"}),
    )?;
    println!("VSM / RT visibility MAE: {mae:.6}");
    ensure!(
        mae < 0.08,
        "VSM visibility differs excessively from RT reference"
    );
    let mut moved = lights;
    moved[0].revision += 1;
    shadows.update(&mut gpu, &mesh, &moved, Vec3::ZERO, false)?;
    let list = shadows.record(&mut gpu, &mesh)?;
    let point = gpu.submit(list, &[])?;
    shadows.submitted(point);
    gpu.resolve(point)?;
    ensure!(
        shadows.stats(&mut gpu, &mesh)?.dirty_pages > 0,
        "light revision did not invalidate VSM pages"
    );
    let position = Vec3::new(0.0, 5.0, 4.0);
    let spot = vec![ara_vsm::spot(2, position, -position, 2.1, 50.0, 1)?];
    let cube = ara_vsm::point(3, position, 50.0, 1)?.to_vec();
    for (name, lights) in [("spot", spot), ("cube", cube)] {
        shadows.update(&mut gpu, &mesh, &lights, position, true)?;
        let list = shadows.record(&mut gpu, &mesh)?;
        let p = gpu.submit(list, &[])?;
        shadows.submitted(p);
        gpu.resolve(p)?;
        let stats = shadows.stats(&mut gpu, &mesh)?;
        ensure!(
            stats.dirty_pages > 0 && stats.missing_pages == 0 && stats.cluster_overflow == 0,
            "local-light VSM capacity/mark failure"
        );
        shadows.capture(&mut gpu, &mesh, &capture.join(format!("{name}.png")))?;
    }
    reference.retire(&mut gpu)?;
    shadows.retire(&mut gpu)?;
    mesh.retire(&mut gpu)?;
    let mut indirect = GeometryView::new(
        &mut gpu,
        &geometry,
        &instances,
        size,
        10000,
        &root.join("cooked"),
        false,
    )?;
    let fallback = draw(&mut gpu, &mut indirect, &camera, false, false)?;
    indirect.capture(&mut gpu, &capture.join("indirect.png"))?;
    coverage_bound(&fallback, &hybrid, [640, 360])?;
    coverage_bound(&hybrid, &fallback, [640, 360])?;
    gpu.assert_clean()?;
    indirect.retire(&mut gpu)?;
    let capacity = geometry.clusters.len() as u32 / 2;
    let mut streamed = GeometryView::new_with_pool(
        &mut gpu,
        &geometry,
        &instances,
        GeometryConfig {
            size,
            visible_clusters: 10000,
            resident_pages: capacity,
            mesh_shaders: true,
        },
        &root.join("cooked"),
    )?;
    draw(&mut gpu, &mut streamed, &camera, false, true)?;
    let uploaded = streamed.prepare_streaming(&mut gpu, &geometry)?;
    ensure!(
        uploaded > 0 && uploaded <= 16,
        "GPU feedback did not request bounded page uploads"
    );
    draw(&mut gpu, &mut streamed, &camera, false, true)?;
    streamed.capture(&mut gpu, &capture.join("streamed.png"))?;
    let stats = streamed.stats(&mut gpu)?;
    ensure!(
        stats.pages_resident <= capacity && stats.resident_floor > 0 && stats.overflow == 0,
        "bounded pool did not report its coarse fallback"
    );
    gpu.assert_clean()?;
    streamed.retire(&mut gpu)?;
    Ok(())
}
