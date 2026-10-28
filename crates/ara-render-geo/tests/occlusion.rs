use anyhow::{Result, ensure};
use ara_geo::{Settings, cook, sample};
use ara_gpu::GpuDevice;
use ara_render::scene::CameraSpec;
use ara_render_geo::{DebugView, GeometryView, Instance, View};
use glam::Vec3;
use std::path::Path;

fn frame(
    gpu: &mut GpuDevice,
    view: &mut GeometryView,
    camera: &CameraSpec,
    reset: bool,
) -> Result<()> {
    view.update(
        gpu,
        &View {
            camera,
            error_pixels: 1.0,
            reset_history: reset,
            debug: DebugView::Clusters,
            hardware_only: false,
            full_resolution: false,
        },
    )?;
    let list = view.record(gpu)?;
    let point = gpu.submit(list, &[])?;
    view.submitted(point);
    gpu.resolve(point)?;
    Ok(())
}
#[test]
fn previous_hzb_defers_and_current_hzb_recovers_disoccluded_geometry() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capture = root.join("captures/occlusion");
    std::fs::create_dir_all(&capture)?;
    let g = cook(&sample::rock(48, 96), Settings::default())?;
    let instances = [
        Instance {
            position_scale: [0.0, 0.0, 0.0, 3.0],
            color_roughness: [0.5, 0.3, 0.2, 0.5],
            identity: [1, 0, 0, 0],
        },
        Instance {
            position_scale: [0.0, 0.0, -5.0, 0.35],
            color_roughness: [0.2, 0.5, 0.3, 0.5],
            identity: [2, 0, 0, 0],
        },
    ];
    let mut gpu = GpuDevice::new(&[], capture.join("gpu"))?;
    // Atomic visibility needs 64-bit buffer atomics (Metal has none yet): skip with the
    // reason `GeometryView` gives, like the product path.
    if !gpu.info().buffer_int64_atomics {
        eprintln!(
            "skipped: ARA_GEO_CAPABILITY: shaderBufferInt64Atomics required for atomic visibility"
        );
        return Ok(());
    }
    let mut view = GeometryView::new(
        &mut gpu,
        &g,
        &instances,
        [640, 360],
        10000,
        &root.join("cooked"),
        true,
    )?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0.0, 0.0, 8.0),
        target: Vec3::ZERO,
        vertical_fov: 55.0f32.to_radians(),
    };
    frame(&mut gpu, &mut view, &camera, true)?;
    frame(&mut gpu, &mut view, &camera, false)?;
    let hidden = view.stats(&mut gpu)?;
    ensure!(
        hidden.instances_occluded_previous > 0 && hidden.instances_occluded_current > 0,
        "fixture did not exercise both HZB tests"
    );
    let moved = CameraSpec::LookAt {
        position: Vec3::new(12.0, 2.0, -3.0),
        target: Vec3::new(0.0, 0.0, -3.0),
        vertical_fov: 55.0f32.to_radians(),
    };
    frame(&mut gpu, &mut view, &moved, false)?;
    let visible = view.identities(&mut gpu)?;
    ensure!(
        visible.iter().any(|id| id[0] == 2),
        "post pass lost a disoccluded rear instance"
    );
    let recovered = view.stats(&mut gpu)?;
    ensure!(
        recovered.instances_occluded_previous > 0 && recovered.instances_occluded_current == 0,
        "disocclusion did not reach the current-HZB retest"
    );
    view.capture(&mut gpu, &capture.join("post-recovery.png"))?;
    gpu.assert_clean()?;
    view.retire(&mut gpu)?;
    Ok(())
}
