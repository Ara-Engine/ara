//! World-space labels remain native UI controls, positioned by the current camera.
use super::*;
use ara_runtime::{components::Camera, sync::WorldPose};
use ara_sim::bevy_ecs::entity::Entity;
use glam::Vec3;

pub(super) fn update(
    ui: &mut Ui,
    runtime: &Runtime,
    binding: &ara_play::UiBinding,
    entity: Option<Entity>,
    owner: Option<Entity>,
    size: [u32; 2],
) -> Result<()> {
    let node = ui.find(&binding.node).context("validated world label")?;
    let pose = entity.and_then(|e| runtime.world().get::<WorldPose>(e));
    let active = entity != owner
        && entity.is_some_and(|e| {
            binding.active.as_ref().is_none_or(|field| {
                ara_runtime_script::read(runtime.world(), e, &binding.component)
                    .and_then(|v| v.get(field).and_then(Value::as_bool))
                    .unwrap_or(false)
            })
        });
    let camera = runtime
        .world()
        .iter_entities()
        .filter_map(|e| Some((e.get::<Camera>()?, e.get::<WorldPose>()?)))
        .min_by_key(|(c, _)| c.priority);
    let projected = pose.zip(camera).and_then(|(pose, (camera, eye))| {
        let forward = (eye.0.rotation * Vec3::NEG_Z).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);
        let delta = pose.0.translation + Vec3::Y * 2.4 - eye.0.translation;
        let depth = delta.dot(forward);
        if depth <= 0.1 {
            return None;
        }
        let tangent = (camera.fov.to_radians() * 0.5).tan();
        let x = delta.dot(right) / (depth * tangent * size[0] as f32 / size[1].max(1) as f32);
        let y = delta.dot(up) / (depth * tangent);
        (x.abs() < 1. && y.abs() < 1.).then_some([
            ((x * 0.5 + 0.5) * size[0] as f32 - 90.).clamp(0., (size[0] as f32 - 180.).max(0.)),
            (0.5 - y * 0.5) * size[1] as f32,
        ])
    });
    let projected = projected.map(|[x, y]| [x, y.clamp(0., (size[1] as f32 - 28.).max(0.))]);
    let covered = projected.is_some_and(|[x, y]| {
        ["ui_001", "ui_008", "help"].into_iter().any(|name| {
            ui.find(name).is_some_and(|panel| {
                let panel = &ui.nodes[panel];
                let r = panel.rect;
                panel.visible && x < r.x + r.w && x + 180. > r.x && y < r.y + r.h && y + 28. > r.y
            })
        })
    });
    ui.set_visible(node, active && projected.is_some() && !covered)
        .map_err(ui_error)?;
    if let Some([x, y]) = projected {
        ui.set_style(
            node,
            &format!("position:absolute;left:{x:.0}px;top:{y:.0}px"),
        )
        .map_err(ui_error)?;
    }
    Ok(())
}
