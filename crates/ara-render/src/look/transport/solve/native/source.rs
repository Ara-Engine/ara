//! Fixed source quadrature for CPU patch forcing. Visibility is multiplied per
//! source sample, before integration. This is separate from B1's pixel shader.
use super::*;
use ara_frame::scene::{LIGHT_DISC, LIGHT_RECT, LIGHT_SHAPED, LIGHT_SPOT, Light};
const SAMPLES: u32 = 64;

pub(super) fn light_gain(light: &Light) -> Vec3 {
    Vec3::from_slice(&light.color_lumens[..3]) * light.color_lumens[3]
}

/// Thin barn-door flaps clip a rectangle's visible source interval exactly.
fn span(extent: f32, offset: f32, height: f32, doors: [f32; 2]) -> [f32; 2] {
    if doors[1] <= 0. {
        return [-extent, extent];
    }
    if height <= doors[1] {
        return if offset.abs() <= extent + doors[0] * height / doors[1] {
            [-extent, extent]
        } else {
            [1., -1.]
        };
    }
    let k = doors[1] / height;
    let edge = extent + doors[0];
    [
        (-extent).max((-edge - offset * k) / (1. - k)),
        extent.min((edge - offset * k) / (1. - k)),
    ]
}

pub(super) fn lamp(
    light: &Light,
    cpu: &CpuTrace<'_>,
    materials: &NativeMaterials<'_>,
    position: Vec3,
    normal: Vec3,
) -> Vec3 {
    let centre = Vec3::from_slice(&light.position_range[..3]);
    let offset = position - centre;
    let distance = offset.length();
    let range = light.position_range[3];
    if distance <= 1e-5 || distance >= range || light.color_lumens[3] == 0. {
        return Vec3::ZERO;
    }
    let kind = light.parameters[0] % LIGHT_SHAPED;
    let axis = Vec3::from_slice(&light.direction_outer[..3]).normalize_or_zero();
    let width = Vec3::from_slice(&light.tangent_length[..3]);
    let area = kind >= LIGHT_RECT || light.parameters[2] > 0.;
    let count = if area { SAMPLES } else { 1 };
    let radius = light.parameters[2];
    let facing = offset / distance;
    let tangent = if kind >= LIGHT_RECT {
        width
    } else {
        facing.any_orthonormal_vector()
    };
    let bitangent = if kind >= LIGHT_RECT {
        tangent.cross(axis)
    } else {
        facing.cross(tangent)
    };
    let ex = light.rect[0].max(0.001);
    let ey = light.rect[1].max(0.001);
    let sx = span(
        ex,
        offset.dot(tangent),
        offset.dot(axis),
        [light.rect[2], light.rect[3]],
    );
    let sy = span(
        ey,
        offset.dot(bitangent),
        offset.dot(axis),
        [light.rect[2], light.rect[3]],
    );
    let rect_share = ((sx[1] - sx[0]).max(0.) * (sy[1] - sy[0]).max(0.)) / (4. * ex * ey);
    if kind >= LIGHT_RECT && offset.dot(axis) <= 0. {
        return Vec3::ZERO;
    }
    let window = (1. - (distance / range).powi(4)).max(0.).powi(2);
    let mut integral = Vec3::ZERO;
    for k in 0..count {
        let sample = if kind == LIGHT_RECT {
            centre
                + tangent * (sx[0] + (sx[1] - sx[0]) * ((k % 8) as f32 + 0.5) / 8.)
                + bitangent * (sy[0] + (sy[1] - sy[0]) * ((k / 8) as f32 + 0.5) / 8.)
        } else if area {
            // Fixed projected-disc quadrature; a finite sphere uses its tangent
            // silhouette. Tube samples additionally span the authored centreline.
            let silhouette = if kind == LIGHT_DISC {
                1.
            } else {
                distance / (distance * distance - radius * radius).max(1e-8).sqrt()
            };
            let r = radius * silhouette * ((k as f32 + 0.5) / count as f32).sqrt();
            let phi = k as f32 * 2.3999631;
            centre
                + (tangent * phi.cos() + bitangent * phi.sin()) * r
                + if kind < LIGHT_RECT {
                    width * (((k as f32 * 0.618034 + 0.5).fract() - 0.5) * light.tangent_length[3])
                } else {
                    Vec3::ZERO
                }
        } else {
            centre
        };
        let delta = sample - position;
        let r = delta.length();
        if r <= 1e-5 {
            continue;
        }
        let direction = delta / r;
        let mut weight = normal.dot(direction).max(0.);
        if kind == LIGHT_SPOT {
            let t = ((-direction.dot(axis) - light.direction_outer[3])
                / (light.parameters[1] - light.direction_outer[3]).max(1e-4))
            .clamp(0., 1.);
            weight *= t * t * (3. - 2. * t);
        }
        if kind >= LIGHT_RECT {
            weight *= (-direction.dot(axis)).max(0.);
        }
        if kind == LIGHT_RECT {
            weight *= rect_share;
        }
        if weight > 0. {
            integral += visibility(cpu, materials, position, direction, (r - 1e-4).max(1e-4))
                * (weight / (r * r).max(0.01));
        }
    }
    light_gain(light) * integral * (window / (4. * std::f32::consts::PI * count as f32))
}
