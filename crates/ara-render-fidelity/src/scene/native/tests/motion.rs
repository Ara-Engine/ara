use super::*;
use crate::{FidelityView, FrameInput};

pub(super) fn check(
    gpu: &mut VulkanDevice,
    view: &mut FidelityView,
    scene: &mut NativeScene,
    camera: &ara_render::scene::CameraSpec,
    size: [u32; 2],
    output: &std::path::Path,
) -> Result<()> {
    let resources: Vec<_> = view
        .scene
        .native
        .as_ref()
        .expect("native view")
        .pool
        .as_ref()
        .expect("pool")
        .resources()
        .collect();
    scene.instances[1].world.w_axis.x += 0.3;
    view.update_native_scene(gpu, scene)?;
    assert_eq!(view.scene.acceleration_update().rebuilt_blas, 0);
    assert_eq!(view.scene.acceleration_update().reused_blas, 1);
    assert!(view.scene.acceleration_update().tlas_rebuilt);
    let current: Vec<_> = view
        .scene
        .native
        .as_ref()
        .expect("native view")
        .pool
        .as_ref()
        .expect("pool")
        .resources()
        .collect();
    assert_eq!(
        resources, current,
        "rigid motion must retain vertex/index allocations"
    );
    let input = |tick, reset| FrameInput {
        camera,
        tick,
        delta_seconds: 1.0 / 60.0,
        reset,
        lighting_changed: false,
        deterministic: true,
    };
    view.render(gpu, &input(1, true))?;
    let resolved = camera.resolve(size)?;
    let clip = resolved.projection * resolved.view * scene.instances[1].world.w_axis;
    let x = ((clip.x / clip.w * 0.5 + 0.5) * size[0] as f32) as usize;
    let y = ((-clip.y / clip.w * 0.5 + 0.5) * size[1] as f32) as usize;
    let pixel = y * size[0] as usize + x;
    let moving = view.capture_motion(gpu)?[pixel];
    assert!(
        moving[0] < -0.03 && moving[1].abs() < 1e-6,
        "object motion absent or wrong: {moving:?}"
    );
    view.capture(gpu)?
        .save(output.join("native-rigid-moved.png"))?;
    view.render(gpu, &input(2, false))?;
    let steady = view.capture_motion(gpu)?[pixel];
    assert!(
        steady[0].abs() < 1e-6 && steady[1].abs() < 1e-6,
        "settled render retained stale motion: {steady:?}"
    );
    let table = view.scene.table;
    view.update_native_scene(gpu, scene)?;
    assert_eq!(
        table, view.scene.table,
        "idle snapshots must not allocate or upload"
    );
    assert!(!view.scene.acceleration_update().tlas_rebuilt);
    let good_world = scene.instances[1].world;
    scene.instances[1].world.w_axis.x = f32::NAN;
    assert!(view.update_native_scene(gpu, scene).is_err());
    assert_eq!(
        table, view.scene.table,
        "invalid poses must preserve last good bindings"
    );
    scene.instances[1].world = good_world;
    scene.materials[1].pbr.emissive_roughness[1] = 0.0;
    view.update_native_scene(gpu, scene)?;
    assert!(
        !view.scene.acceleration_update().tlas_rebuilt,
        "emission factors do not rebuild geometry"
    );
    view.render(gpu, &input(3, true))?;
    let off = view.capture_radiance(gpu)?[pixel];
    assert!(
        off[..3].iter().all(|v| v.abs() < 1e-6),
        "live emitter disable did not apply: {off:?}"
    );
    Ok(())
}
