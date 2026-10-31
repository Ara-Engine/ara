//! Extracted local lights (`ara_runtime::extract::LightView`) as renderer records
//! (`ara_render::scene::Light`), the one conversion the raster and fidelity paths share.
use ara_render::scene::{self, LIGHT_DISC, LIGHT_POINT, LIGHT_RECT, LIGHT_SPOT};
use ara_runtime::{components::LightKind, extract::LightView};

/// The render record of `light`: its kind, cone, emitter size and barn doors. Tubes
/// keep at least a millimetre of radius, which marks them as area sources.
pub(crate) fn scene_light(light: &LightView) -> scene::Light {
    let [x, y, z] = light.position;
    let [dx, dy, dz] = light.direction;
    let [tx, ty, tz] = light.tangent;
    let spot = light.kind == LightKind::Spot;
    let kind = match light.kind {
        LightKind::Spot => LIGHT_SPOT,
        LightKind::Rect => LIGHT_RECT,
        LightKind::Disc => LIGHT_DISC,
        LightKind::Point | LightKind::Directional => LIGHT_POINT,
    };
    let point = matches!(light.kind, LightKind::Point | LightKind::Spot);
    let length = if point { light.length } else { 0. };
    let radius = match light.kind {
        LightKind::Rect => 0.,
        _ if length > 0. => light.radius.max(0.001),
        _ => light.radius,
    };
    let rect = if light.kind == LightKind::Rect {
        let [angle, door] = light.barn_doors;
        let (sin, cos) = angle.clamp(0., 90.).to_radians().sin_cos();
        let door = door.max(0.);
        [
            light.size[0].max(0.001) * 0.5,
            light.size[1].max(0.001) * 0.5,
            door * sin,
            // Folded flat (90 degrees) has no height: no doors.
            door * cos,
        ]
    } else {
        [0.; 4]
    };
    scene::Light {
        position_range: [x, y, z, light.range],
        color_lumens: [light.color[0], light.color[1], light.color[2], light.lumens],
        direction_outer: [
            dx,
            dy,
            dz,
            if spot {
                light.cone[1].to_radians().cos()
            } else {
                0.
            },
        ],
        parameters: [
            kind,
            if spot {
                light.cone[0].to_radians().cos()
            } else {
                0.
            },
            radius,
            0.,
        ],
        tangent_length: [tx, ty, tz, length],
        rect,
        tables: [0; 4],
    }
}
