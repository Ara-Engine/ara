//! Exact range overlap over the primary capture's covered-pixel domain. The
//! fixture geometry is made of tessellated boxes, so the hit is analytic.
use super::fixture::Fixture;
use anyhow::Result;
use ara_render::capture::Capture;
use glam::{Mat4, Vec3, Vec4};
use std::collections::BTreeMap;

pub struct Coverage {
    pub lpp_lower: f64,
    pub lpp_upper: f64,
    pub receiver_fraction: f64,
    pub unresolved_pixels: u64,
}

pub fn lpp(f: &Fixture, capture: &Capture, size: [u32; 2]) -> Result<Coverage> {
    let camera = f.camera.resolve(size)?;
    let inverse = (camera.projection * camera.view).inverse();
    let objects: BTreeMap<_, _> = f
        .scene
        .instances
        .iter()
        .map(|i| (i.entity, i.world.inverse()))
        .collect();
    let cell = f
        .scene
        .lights
        .iter()
        .map(|l| l.position_range[3])
        .fold(0.1, f32::max);
    let key = |p: Vec3| (p / cell).floor().as_ivec3().to_array();
    let mut grid = BTreeMap::<[i32; 3], Vec<&ara_render::scene::Light>>::new();
    for l in &f.scene.lights {
        grid.entry(key(Vec3::from_slice(&l.position_range)))
            .or_default()
            .push(l);
    }
    let mut covered = 0u64;
    let mut count = 0u64;
    let mut unresolved = 0u64;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let Some(entity) = capture.entity_at(x, y) else {
                continue;
            };
            covered += 1;
            let Some(world) = objects.get(&entity) else {
                unresolved += 1;
                continue;
            };
            let ndc = Vec3::new(
                (x as f32 + 0.5) / size[0] as f32 * 2. - 1.,
                1. - (y as f32 + 0.5) / size[1] as f32 * 2.,
                1.,
            );
            let a = inverse * ndc.extend(1.);
            let origin = a.truncate() / a.w;
            let b = inverse * Vec4::new(ndc.x, ndc.y, 0.5, 1.);
            let direction = (b.truncate() / b.w - origin).normalize();
            let Some(hit) = hit_box(*world, origin, direction) else {
                // Raster edge rules can cover a pixel whose analytical centre
                // ray misses. Keep it in the domain and bound its unknown LPP.
                unresolved += 1;
                continue;
            };
            let p = origin + direction * hit;
            let k = key(p);
            for z in -1..=1 {
                for y in -1..=1 {
                    for x in -1..=1 {
                        if let Some(lights) = grid.get(&[k[0] + x, k[1] + y, k[2] + z]) {
                            for l in lights {
                                if l.color_lumens[3] > 0.
                                    && p.distance_squared(Vec3::from_slice(&l.position_range))
                                        < l.position_range[3].powi(2)
                                {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let active = f
        .scene
        .lights
        .iter()
        .filter(|l| l.color_lumens[3] > 0.)
        .count() as u64;
    let divide = |n: u64| {
        if covered == 0 {
            0.
        } else {
            n as f64 / covered as f64
        }
    };
    Ok(Coverage {
        lpp_lower: divide(count),
        lpp_upper: divide(count + unresolved * active),
        receiver_fraction: covered as f64 / (f64::from(size[0]) * f64::from(size[1])),
        unresolved_pixels: unresolved,
    })
}
fn hit_box(inverse: Mat4, origin: Vec3, direction: Vec3) -> Option<f32> {
    let origin = inverse.transform_point3(origin);
    let d = inverse.transform_vector3(direction);
    let mut near = 0f32;
    let mut far = f32::INFINITY;
    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if origin[i].abs() > 0.5 {
                return None;
            }
        } else {
            let a = (-0.5 - origin[i]) / d[i];
            let b = (0.5 - origin[i]) / d[i];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
    }
    (far >= near).then_some(near)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transformed_receiver_intersection_keeps_world_distance() {
        let inverse = Mat4::from_scale(Vec3::new(10., 0.2, 10.)).inverse();
        assert!((hit_box(inverse, Vec3::new(0., 5., 0.), -Vec3::Y).unwrap() - 4.9).abs() < 1e-5);
        assert!(hit_box(inverse, Vec3::new(6., 5., 0.), -Vec3::Y).is_none());
    }
}
