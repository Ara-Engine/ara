//! Conservative direct-source support for geometry/material edits. Bounce rays
//! alone cannot discover a moving occluder's shadow on a patch.
use super::*;
use std::collections::BTreeSet;

impl PatchTransport {
    pub(super) fn shadow_receivers(
        &self,
        scene: &Scene,
        region: Region,
        affected: &mut BTreeSet<u32>,
    ) {
        let lo = Vec3::from(region.min);
        let hi = Vec3::from(region.max);
        for p in 0..self.graph.links.patches.len() as u32 {
            let position = Vec3::from(self.graph.links.patches.position(p));
            if (scene.sun_lux.max_element() > 0.
                && crosses(
                    position,
                    scene.sun_direction.normalize_or_zero() * self.distance,
                    lo,
                    hi,
                ))
                || scene.lights.iter().any(|light| {
                    if light.color_lumens[3] == 0. {
                        return false;
                    }
                    let delta = Vec3::from_slice(&light.position_range[..3]) - position;
                    let distance = delta.length();
                    if distance >= light.position_range[3] {
                        return false;
                    }
                    let kind = light.parameters[0] % crate::scene::LIGHT_SHAPED;
                    let radius = light.parameters[2];
                    let extent = if kind >= crate::scene::LIGHT_RECT {
                        glam::Vec2::new(light.rect[0], light.rect[1])
                            .length()
                            .max(radius)
                    } else if distance <= radius {
                        return true;
                    } else {
                        radius * distance / (distance * distance - radius * radius).sqrt()
                            + light.tangent_length[3]
                                * 0.5
                                * Vec3::from_slice(&light.tangent_length[..3]).length()
                    };
                    crosses(
                        position,
                        delta,
                        lo - Vec3::splat(extent),
                        hi + Vec3::splat(extent),
                    )
                })
            {
                affected.insert(p);
            }
        }
    }
}

fn crosses(start: Vec3, delta: Vec3, lo: Vec3, hi: Vec3) -> bool {
    let mut near = 0f32;
    let mut far = 1f32;
    for axis in 0..3 {
        if delta[axis].abs() < 1e-20 {
            if start[axis] < lo[axis] || start[axis] > hi[axis] {
                return false;
            }
        } else {
            let a = (lo[axis] - start[axis]) / delta[axis];
            let b = (hi[axis] - start[axis]) / delta[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return false;
            }
        }
    }
    true
}
