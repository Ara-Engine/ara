//! Read-only picking over the same retained native geometry the viewport presents.
use super::SceneView;
use ara_render::instanced::Scene;
use ara_ui::paint::Box2;
use glam::Vec3;
use std::collections::BTreeSet;
mod snap;

/// A native geometry hit and its source draw, borrowed from the presented scene snapshot.
pub(in crate::app) struct SurfaceHit<'a> {
    /// Presented instance identity, including primitives without an ordinary draw.
    pub entity: u64,
    /// Position on the nearest triangle, in world metres.
    pub position: [f32; 3],
    /// World-space geometric triangle normal.
    pub normal: [f32; 3],
    /// Nearest triangle vertex, for vertex-snap gestures.
    pub vertex: [f32; 3],
    /// Exact primitive draw; terrain or foliage may have no ordinary entity draw.
    pub draw: Option<&'a ara_runtime::extract::Draw>,
}
struct Pick {
    position: Vec3,
    normal: Vec3,
    vertex: Vec3,
    entity: u64,
}

impl SceneView {
    /// The retained geometry belongs to this document. A previous scene's
    /// last-good image may stay visible, but cannot accept authoring snap input.
    pub(in crate::app) fn snap_ready(&self, doc: &str) -> bool {
        if self
            .raster
            .as_ref()
            .and_then(ara_runtime_render::RasterView::scene)
            .is_some()
        {
            self.raster_scene.as_deref() == Some(doc)
        } else {
            self.prepared.is_some() && self.fidelity_scene.as_deref() == Some(doc)
        }
    }

    /// The precise native triangle, normal, vertex candidate and source primitive under a pointer.
    pub(in crate::app) fn surface_hit(
        &self,
        camera: &crate::state::Camera,
        rect: Box2,
        pointer: [f32; 2],
    ) -> Option<SurfaceHit<'_>> {
        self.surface_hit_excluding(camera, rect, pointer, &BTreeSet::new())
    }

    /// Pick through selected geometry while retaining the presented target mesh.
    pub(in crate::app) fn surface_hit_excluding(
        &self,
        camera: &crate::state::Camera,
        rect: Box2,
        pointer: [f32; 2],
        excluded: &BTreeSet<u64>,
    ) -> Option<SurfaceHit<'_>> {
        if ![rect.x, rect.y, rect.w, rect.h]
            .into_iter()
            .chain(pointer)
            .all(f32::is_finite)
            || rect.w <= 0.
            || rect.h <= 0.
            || !rect.contains(pointer[0], pointer[1])
        {
            return None;
        }
        let (origin, direction) = camera.ray(
            [rect.w.max(1.) as u32, rect.h.max(1.) as u32],
            [
                (pointer[0] - rect.x) / rect.w * 2. - 1.,
                1. - (pointer[1] - rect.y) / rect.h * 2.,
            ],
        )?;
        self.ray_hit_excluding(origin, direction, excluded)
    }

    /// Native mesh query used by Drop to Surface; no invented ground-plane hit.
    pub(in crate::app) fn ray_hit_excluding(
        &self,
        origin: Vec3,
        direction: Vec3,
        excluded: &BTreeSet<u64>,
    ) -> Option<SurfaceHit<'_>> {
        let hit = pick_where(self.presented_scene()?, origin, direction, |id| {
            !excluded.contains(&id)
        })?;
        let draw = self
            .raster_frame
            .as_ref()
            .and_then(|frame| frame.draws.iter().find(|draw| draw.entity == hit.entity));
        Some(SurfaceHit {
            entity: hit.entity,
            position: hit.position.to_array(),
            normal: hit.normal.to_array(),
            vertex: hit.vertex.to_array(),
            draw,
        })
    }

    fn presented_scene(&self) -> Option<&Scene> {
        self.raster
            .as_ref()
            .and_then(ara_runtime_render::RasterView::scene)
            .or_else(|| self.prepared.as_ref().map(|prepared| &prepared.scene))
    }
}

#[cfg(test)]
fn pick(scene: &Scene, origin: Vec3, direction: Vec3) -> Option<Pick> {
    pick_where(scene, origin, direction, |_| true)
}

fn pick_where(
    scene: &Scene,
    origin: Vec3,
    direction: Vec3,
    accept: impl Fn(u64) -> bool,
) -> Option<Pick> {
    if !origin.is_finite() || !direction.is_finite() || direction.length_squared() < f32::EPSILON {
        return None;
    }
    let mut nearest = f32::INFINITY;
    let mut hit = None;
    for instance in &scene.instances {
        if !accept(instance.entity) {
            continue;
        }
        let Some(geometry) = scene.geometry.get(instance.geometry) else {
            continue;
        };
        if !instance.world.is_finite() || instance.world.determinant().abs() < 1e-20 {
            continue;
        }
        let inverse = instance.world.inverse();
        let local_origin = inverse.transform_point3(origin);
        // Keep direction unnormalized: the triangle parameter remains world-ray distance,
        // even under mirrored/nonuniform instance scales.
        let local_direction = inverse.transform_vector3(direction);
        if !bounds_hit(geometry.bounds, local_origin, local_direction, nearest) {
            continue;
        }
        let Some(lod) = geometry.lods.first() else {
            continue;
        };
        for triangle in lod.indices.as_chunks::<3>().0 {
            let Some(a) = geometry.vertices.get(triangle[0] as usize) else {
                continue;
            };
            let Some(b) = geometry.vertices.get(triangle[1] as usize) else {
                continue;
            };
            let Some(c) = geometry.vertices.get(triangle[2] as usize) else {
                continue;
            };
            if let Some(distance) = triangle_hit(
                local_origin,
                local_direction,
                Vec3::from(a.position),
                Vec3::from(b.position),
                Vec3::from(c.position),
            ) && distance < nearest
            {
                let a = instance.world.transform_point3(Vec3::from(a.position));
                let b = instance.world.transform_point3(Vec3::from(b.position));
                let c = instance.world.transform_point3(Vec3::from(c.position));
                let point = origin + direction * distance;
                let vertex = [a, b, c].into_iter().min_by(|a, b| {
                    a.distance_squared(point)
                        .total_cmp(&b.distance_squared(point))
                })?;
                let normal = (b - a).cross(c - a).normalize_or_zero();
                if point.is_finite() && normal.is_finite() && vertex.is_finite() {
                    nearest = distance;
                    hit = Some(Pick {
                        position: point,
                        normal,
                        vertex,
                        entity: instance.entity,
                    });
                }
            }
        }
    }
    hit
}

fn bounds_hit(bounds: [Vec3; 2], origin: Vec3, direction: Vec3, nearest: f32) -> bool {
    if !origin.is_finite() || !direction.is_finite() || !bounds.iter().all(|b| b.is_finite()) {
        return false;
    }
    let mut enter = 0_f32;
    let mut exit = nearest;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-20 {
            if origin[axis] < bounds[0][axis] || origin[axis] > bounds[1][axis] {
                return false;
            }
        } else {
            let a = (bounds[0][axis] - origin[axis]) / direction[axis];
            let b = (bounds[1][axis] - origin[axis]) / direction[axis];
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if exit < enter {
                return false;
            }
        }
    }
    exit > 0.
}

fn triangle_hit(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let ab = b - a;
    let ac = c - a;
    let cross = direction.cross(ac);
    let determinant = ab.dot(cross);
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return None;
    }
    let offset = origin - a;
    let u = offset.dot(cross) / determinant;
    if !(0. ..=1.).contains(&u) {
        return None;
    }
    let q = offset.cross(ab);
    let v = direction.dot(q) / determinant;
    if v < 0. || u + v > 1. {
        return None;
    }
    let distance = ac.dot(q) / determinant;
    (distance.is_finite() && distance > 0.0001).then_some(distance)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ara_render::instanced::{Geometry, Instance, Lod, Vertex};
    use glam::Mat4;
    pub(super) fn scene(worlds: &[Mat4]) -> Scene {
        Scene {
            geometry: vec![Geometry {
                key: [1; 32],
                vertices: [[-1., -1., 0.], [1., -1., 0.], [0., 1., 0.]]
                    .map(|position| Vertex {
                        position,
                        normal: [0., 0., 1.],
                        uv: [0.; 2],
                        tangent: [1., 0., 0., 1.],
                        uv1: [0.; 2],
                        terrain_weights: 0,
                        reserved: 0,
                    })
                    .into(),
                lods: vec![Lod {
                    indices: vec![0, 1, 2],
                    error: 0.,
                }],
                bounds: [Vec3::new(-1., -1., 0.), Vec3::new(1., 1., 0.)],
                deforming: false,
                revision: 0,
            }],
            instances: worlds
                .iter()
                .enumerate()
                .map(|(id, world)| Instance {
                    entity: id as u64,
                    name: format!("Triangle {id}"),
                    geometry: 0,
                    material: 0,
                    world: *world,
                    previous_world: *world,
                })
                .collect(),
            materials: Vec::new(),
            textures: 0,
            lights: Vec::new(),
            sun_direction: Vec3::Y,
            sun_lux: Vec3::ONE,
            environment: Vec::new(),
            environment_size: [0; 2],
        }
    }
    #[test]
    fn nearest_shared_triangle_respects_nonuniform_mirrored_instances() {
        let scene = scene(&[
            Mat4::from_translation(Vec3::new(0., 0., -5.)),
            Mat4::from_scale_rotation_translation(
                Vec3::new(-2., 3., 0.5),
                glam::Quat::IDENTITY,
                Vec3::new(0., 0., -2.),
            ),
        ]);
        let hit = pick(&scene, Vec3::ZERO, -Vec3::Z).unwrap();
        assert!((hit.position - Vec3::new(0., 0., -2.)).length() < 0.0001);
        assert_eq!(hit.entity, 1);
        assert!(hit.normal.is_finite() && hit.vertex.is_finite());
        assert!(pick(&scene, Vec3::new(10., 0., 0.), -Vec3::Z).is_none());
    }
    #[test]
    fn excluding_source_instances_picks_the_real_target_behind_them() {
        let scene = scene(&[
            Mat4::from_translation(-Vec3::Z * 5.),
            Mat4::from_translation(-Vec3::Z * 2.),
        ]);
        let target = pick_where(&scene, Vec3::ZERO, -Vec3::Z, |id| id != 1).unwrap();
        assert_eq!(target.entity, 0);
        assert_eq!(target.position, Vec3::new(0., 0., -5.));
        assert!(pick_where(&scene, Vec3::ZERO, -Vec3::Z, |_| false).is_none());
    }
    #[test]
    fn stale_indices_singular_transforms_parallel_and_nonfinite_rays_are_safe() {
        let mut scene = scene(&[Mat4::ZERO, Mat4::IDENTITY]);
        scene.geometry[0].lods[0].indices = vec![0, u32::MAX, 2, 0, 1];
        assert!(pick(&scene, Vec3::Z, -Vec3::Z).is_none());
        assert!(pick(&scene, Vec3::Z, Vec3::X).is_none());
        assert!(pick(&scene, Vec3::splat(f32::NAN), -Vec3::Z).is_none());
        assert!(pick(&scene, Vec3::Z, Vec3::ZERO).is_none());
    }
    #[test]
    fn orthographic_pointer_rays_pick_off_centre_triangles_at_near_and_far_depths() {
        let camera = crate::state::Camera {
            target: Vec3::ZERO,
            yaw: 0.,
            pitch: 0.,
            distance: 5.,
            projection: crate::state::CameraProjection::Orthographic,
            orthographic_height: 4.,
            ..Default::default()
        };
        let geometry = scene(&[
            Mat4::from_translation(Vec3::new(0., 0., -20.)),
            Mat4::IDENTITY,
        ]);
        let (origin, direction) = camera.ray([400, 400], [0.2, 0.]).unwrap();
        let hit = pick(&geometry, origin, direction).unwrap();
        assert_eq!(hit.entity, 1);
        assert!((hit.position - Vec3::new(0.4, 0., 0.)).length() < 1e-4);
        let far = pick(
            &scene(&[Mat4::from_translation(Vec3::new(0., 0., -20.))]),
            origin,
            direction,
        )
        .unwrap();
        assert!((far.position - Vec3::new(0.4, 0., -20.)).length() < 1e-4);
        let (miss, direction) = camera.ray([400, 400], [0.9, 0.]).unwrap();
        assert!(pick(&geometry, miss, direction).is_none());
    }
    #[test]
    #[ignore = "explicit surface-picking CPU benchmark; writes only target/ evidence"]
    fn surface_pick_10000_instances_benchmark() {
        let transforms: Vec<_> = (0..10_000)
            .map(|index| {
                Mat4::from_translation(Vec3::new(
                    (index % 100) as f32 * 10. - 500.,
                    (index / 100) as f32 * 10. - 500.,
                    -5.,
                ))
            })
            .collect();
        let scene = scene(&transforms);
        let mut samples = Vec::new();
        for _ in 0..200 {
            let started = std::time::Instant::now();
            let hit =
                std::hint::black_box(pick(std::hint::black_box(&scene), Vec3::ZERO, -Vec3::Z));
            samples.push(started.elapsed().as_secs_f64() * 1000.);
            assert!(hit.is_some());
        }
        samples.sort_by(f64::total_cmp);
        let report = serde_json::json!({"instances":10_000,"queries":samples.len(),"p50Ms":samples[samples.len()/2],"p95Ms":samples[samples.len()*95/100],"maxMs":samples.last()});
        let output = crate::test_target().join("ux-picking-bench.json");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(output, ara_json::to_canonical_string(&report)).unwrap();
        println!("{}", ara_json::to_canonical_string(&report));
    }
}
