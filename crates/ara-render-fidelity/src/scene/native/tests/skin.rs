use super::*;
use crate::{FidelityView, FrameInput};

pub(super) fn check(
    gpu: &mut GpuDevice,
    view: &mut FidelityView,
    scene: &mut NativeScene,
    camera: &ara_render::scene::CameraSpec,
    size: [u32; 2],
    output: &std::path::Path,
) -> Result<()> {
    let pool = view
        .scene
        .native
        .as_ref()
        .expect("native")
        .pool
        .as_ref()
        .expect("pool");
    let static_buffer = pool.resources().next();
    let original_table = pool.table_index();
    scene.materials[1].pbr.emissive_roughness[1] = 3.0;
    // Two extractions before presentation must retain the last presented vertices,
    // rather than treating the intermediate CPU/GPU pose as velocity history.
    for _ in 0..2 {
        for vertex in &mut scene.geometry[0].vertices {
            vertex.position[0] += 0.15;
        }
        for bound in &mut scene.geometry[0].bounds {
            bound.x += 0.15;
        }
        scene.geometry[0].revision += 1;
        view.update_native_scene(gpu, scene)?;
        assert_eq!(view.scene.acceleration_update().rebuilt_blas, 1);
        assert_eq!(view.scene.acceleration_update().refit_fallbacks, 1);
        assert!(view.scene.acceleration_update().tlas_rebuilt);
    }
    let pool = view
        .scene
        .native
        .as_ref()
        .expect("native")
        .pool
        .as_ref()
        .expect("pool");
    assert_eq!(
        static_buffer,
        pool.resources().next(),
        "skin poses must retain static indices"
    );
    assert_ne!(original_table, pool.table_index());
    let table = view.scene.table;
    scene.geometry[0].vertices[0].position[0] = f32::NAN;
    scene.geometry[0].revision += 1;
    assert!(view.update_native_scene(gpu, scene).is_err());
    assert_eq!(
        table, view.scene.table,
        "invalid skin must preserve published bindings"
    );
    scene.geometry[0].vertices[0].position[0] = -0.4 + 0.3;
    let input = |tick| FrameInput {
        camera,
        tick,
        delta_seconds: 1.0 / 60.0,
        reset: false,
        lighting_changed: false,
        deterministic: true,
    };
    view.render(gpu, &input(4))?;
    let centre = scene.instances[1]
        .world
        .transform_point3(Vec3::new(0.3, 0.0, 0.0));
    let resolved = camera.resolve(size)?;
    let clip = resolved.projection * resolved.view * centre.extend(1.0);
    let x = ((clip.x / clip.w * 0.5 + 0.5) * size[0] as f32) as usize;
    let y = ((-clip.y / clip.w * 0.5 + 0.5) * size[1] as f32) as usize;
    let pixel = y * size[0] as usize + x;
    let moving = view.capture_motion(gpu)?[pixel];
    assert!(
        moving[0] < -0.03 && moving[1].abs() < 1e-6,
        "skin velocity must use presented pose: {moving:?}"
    );
    assert!(
        view.capture_radiance(gpu)?[pixel][1] > 1.0,
        "updated skin BLAS/shading mismatch"
    );
    view.capture(gpu)?
        .save(output.join("native-skin-moved.png"))?;
    view.render(gpu, &input(5))?;
    let steady = view.capture_motion(gpu)?[pixel];
    assert!(
        steady[..2].iter().all(|v| v.abs() < 1e-6),
        "settled skin retained velocity: {steady:?}"
    );
    // A later successful publication must also work after the rejected pose.
    view.update_native_scene(gpu, scene)?;
    view.render(gpu, &input(6))?;
    Ok(())
}
