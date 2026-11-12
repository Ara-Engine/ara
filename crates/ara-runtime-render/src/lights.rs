//! Extracted local lights (`ara_runtime::extract::LightView`) as renderer records
//! (`ara_frame::scene::Light`), the one conversion the raster and fidelity paths share.
use ara_frame::scene::{self, LIGHT_DISC, LIGHT_POINT, LIGHT_RECT, LIGHT_SHAPED, LIGHT_SPOT};
use ara_runtime::{components::LightKind, extract::LightView, light_profile::LightProfile};

/// The render record of `light`: its kind, cone, emitter size, barn doors and fog
/// scattering intensity. Tubes keep at least a millimetre of radius, which marks them
/// as area sources.
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
            // w: the light's volumetric fog scattering intensity.
            light.volumetric,
        ],
        tangent_length: [tx, ty, tz, length],
        rect,
        tables: [0; 4],
    }
}

/// The light-table words `lights`' profiles occupy, and each light's profile offset. Word 0 is
/// reserved, so a zero offset means none. A profile is `vertical`, `horizontal` (u32),
/// then its intensities over the peak (f32 bits, azimuth-major), stored once however
/// many lights share it.
pub(crate) fn tables(lights: &[LightView]) -> (Vec<u32>, Vec<u32>) {
    let mut words = vec![0u32];
    let mut stored: Vec<(*const LightProfile, u32)> = Vec::new();
    let offsets = lights
        .iter()
        .map(|light| {
            let Some(profile) = &light.profile else {
                return 0;
            };
            let key = std::sync::Arc::as_ptr(profile);
            if let Some(&(_, offset)) = stored.iter().find(|(p, _)| *p == key) {
                return offset;
            }
            let offset = words.len() as u32;
            words.extend([profile.vertical, profile.horizontal]);
            words.extend(profile.values.iter().map(|v| v.to_bits()));
            stored.push((key, offset));
            offset
        })
        .collect();
    (words, offsets)
}

/// [`scene_light`] for every light, with its profile offset from [`tables`].
pub(crate) fn scene_lights(lights: &[LightView]) -> Vec<scene::Light> {
    let (_, offsets) = tables(lights);
    lights
        .iter()
        .zip(offsets)
        .map(|(light, profile)| {
            let mut record = scene_light(light);
            if profile != 0 {
                record.tables[0] = profile;
                // Points and spots read their shape row only when marked.
                if record.parameters[0] < LIGHT_RECT {
                    record.parameters[0] += LIGHT_SHAPED;
                }
            }
            record
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn profiles_store_once_and_mark_points_shaped() {
        let profile = Arc::new(LightProfile {
            vertical: 2,
            horizontal: 1,
            peak: 10.,
            lumens: 1.,
            values: vec![1., 0.25],
        });
        let shaped = LightView {
            kind: LightKind::Point,
            direction: [0., -1., 0.],
            tangent: [1., 0., 0.],
            range: 5.,
            profile: Some(profile.clone()),
            ..LightView::default()
        };
        let rect = LightView {
            kind: LightKind::Rect,
            size: [1., 1.],
            ..shaped.clone()
        };
        let plain = LightView {
            profile: None,
            ..shaped.clone()
        };
        let lights = [shaped, plain, rect];
        let (words, offsets) = tables(&lights);
        assert_eq!(offsets, [1, 0, 1], "one copy, shared; zero for none");
        assert_eq!(words, [0, 2, 1, 1f32.to_bits(), 0.25f32.to_bits()]);
        let records = scene_lights(&lights);
        assert_eq!(records[0].parameters[0], LIGHT_POINT + LIGHT_SHAPED);
        assert_eq!(records[1].parameters[0], LIGHT_POINT);
        assert_eq!(
            records[2].parameters[0], LIGHT_RECT,
            "rects always read their shape"
        );
        assert_eq!(records[2].rect[..2], [0.5, 0.5]);
        assert!(records.iter().all(scene::Light::is_valid));
    }
}
