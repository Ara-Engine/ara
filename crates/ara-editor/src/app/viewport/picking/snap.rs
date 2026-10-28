//! Exact rendered vertices for snap gestures; files and cooked assets stay out of input.
use super::*;
use crate::state::Camera;
use glam::Vec2;

/// An actual mesh vertex and its transformed authored normal.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct VertexHit {
    /// The presented source instance, suitable for channel/authoring validation.
    pub entity: u64,
    /// World-space vertex position.
    pub position: Vec3,
    /// World-space mesh vertex normal; never substituted with a ground normal.
    pub normal: Vec3,
}

impl SceneView {
    /// Capture referenced vertices once at gesture start, including selected descendants.
    pub(in crate::app) fn snap_vertices(&self, accept: impl Fn(u64) -> bool) -> Vec<VertexHit> {
        self.presented_scene()
            .map(|scene| vertices(scene, accept))
            .unwrap_or_default()
    }

    /// A visible target vertex within the logical-pixel acquisition radius.
    pub(in crate::app) fn snap_vertex(
        &self,
        camera: &Camera,
        rect: Box2,
        pointer: [f32; 2],
        radius: f32,
        excluded: &BTreeSet<u64>,
    ) -> Option<VertexHit> {
        nearest_vertex(
            self.presented_scene()?,
            camera,
            rect,
            pointer,
            radius,
            excluded,
            None,
        )
    }

    /// Acquire the source from selected mesh vertices, rejecting occluded back vertices.
    pub(in crate::app) fn snap_source_vertex(
        &self,
        camera: &Camera,
        rect: Box2,
        pointer: [f32; 2],
        radius: f32,
        included: &BTreeSet<u64>,
    ) -> Option<VertexHit> {
        nearest_vertex(
            self.presented_scene()?,
            camera,
            rect,
            pointer,
            radius,
            &BTreeSet::new(),
            Some(included),
        )
    }
}

fn visit(
    scene: &Scene,
    accept: impl Fn(u64, [Vec3; 2]) -> bool,
    mut vertex: impl FnMut(VertexHit),
) {
    for instance in &scene.instances {
        if !instance.world.is_finite() || instance.world.determinant().abs() < 1e-20 {
            continue;
        }
        let Some(geometry) = scene.geometry.get(instance.geometry) else {
            continue;
        };
        if !accept(instance.entity, instance.bounds(geometry)) {
            continue;
        }
        let Some(lod) = geometry.lods.first() else {
            continue;
        };
        let normal = instance.world.inverse().transpose();
        // Match native ray picking's primary LOD; stale/unused indices never become targets.
        for index in &lod.indices {
            let Some(value) = geometry.vertices.get(*index as usize) else {
                continue;
            };
            let position = instance.world.transform_point3(Vec3::from(value.position));
            let normal = normal
                .transform_vector3(Vec3::from(value.normal))
                .normalize_or_zero();
            if position.is_finite() && normal.is_finite() {
                vertex(VertexHit {
                    entity: instance.entity,
                    position,
                    normal,
                });
            }
        }
    }
}

fn vertices(scene: &Scene, accept: impl Fn(u64) -> bool) -> Vec<VertexHit> {
    let mut result = Vec::new();
    visit(scene, |id, _| accept(id), |vertex| result.push(vertex));
    result
}

fn nearest_vertex(
    scene: &Scene,
    camera: &Camera,
    rect: Box2,
    pointer: [f32; 2],
    radius: f32,
    excluded: &BTreeSet<u64>,
    included: Option<&BTreeSet<u64>>,
) -> Option<VertexHit> {
    if !radius.is_finite()
        || radius <= 0.
        || !rect.contains(pointer[0], pointer[1])
        || !pointer.into_iter().all(f32::is_finite)
    {
        return None;
    }
    let resolved = camera
        .spec()
        .resolve([rect.w.max(1.) as u32, rect.h.max(1.) as u32])
        .ok()?;
    let clip = resolved.projection * resolved.view;
    let project = |point: Vec3| {
        let p = clip * point.extend(1.);
        if !p.is_finite() || p.w <= 0.01 {
            return None;
        }
        let ndc = p.truncate() / p.w;
        Some([
            rect.x + (ndc.x + 1.) * 0.5 * rect.w,
            rect.y + (1. - ndc.y) * 0.5 * rect.h,
        ])
    };
    // Keep a bounded set of distinct nearby candidates, without allocating per motion event.
    let mut candidates: [Option<(f32, f32, VertexHit)>; 8] = [None; 8];
    visit(
        scene,
        |id, bounds| {
            if excluded.contains(&id) || included.is_some_and(|ids| !ids.contains(&id)) {
                return false;
            }
            let mut low = Vec2::splat(f32::INFINITY);
            let mut high = Vec2::splat(f32::NEG_INFINITY);
            for corner in 0..8 {
                let point = Vec3::new(
                    bounds[usize::from(corner & 1 != 0)].x,
                    bounds[usize::from(corner & 2 != 0)].y,
                    bounds[usize::from(corner & 4 != 0)].z,
                );
                let Some(point) = project(point) else {
                    return true;
                };
                low = low.min(Vec2::from(point));
                high = high.max(Vec2::from(point));
            }
            let p = Vec2::from(pointer);
            p.x >= low.x - radius
                && p.x <= high.x + radius
                && p.y >= low.y - radius
                && p.y <= high.y + radius
        },
        |vertex| {
            let Some(projected) = project(vertex.position) else {
                return;
            };
            let distance = Vec2::from(projected).distance_squared(Vec2::from(pointer));
            if distance > radius * radius {
                return;
            }
            let Some((origin, direction)) = camera.ray(
                [rect.w.max(1.) as u32, rect.h.max(1.) as u32],
                [
                    (projected[0] - rect.x) / rect.w * 2. - 1.,
                    1. - (projected[1] - rect.y) / rect.h * 2.,
                ],
            ) else {
                return;
            };
            let depth = (vertex.position - origin).dot(direction);
            if !depth.is_finite()
                || depth <= 0.
                || candidates.iter().flatten().any(|(_, _, old)| {
                    old.entity == vertex.entity && old.position == vertex.position
                })
            {
                return;
            }
            let at = candidates.iter().position(|candidate| {
                candidate.is_none_or(|(old, old_depth, _)| {
                    distance < old || distance == old && depth < old_depth
                })
            });
            if let Some(at) = at {
                for index in (at + 1..candidates.len()).rev() {
                    candidates[index] = candidates[index - 1];
                }
                candidates[at] = Some((distance, depth, vertex));
            }
        },
    );
    for (_, _, candidate) in candidates.into_iter().flatten() {
        let projected = project(candidate.position)?;
        let (origin, direction) = camera.ray(
            [rect.w.max(1.) as u32, rect.h.max(1.) as u32],
            [
                (projected[0] - rect.x) / rect.w * 2. - 1.,
                1. - (projected[1] - rect.y) / rect.h * 2.,
            ],
        )?;
        let depth = (candidate.position - origin).dot(direction);
        let occluded = pick_where(scene, origin, direction, |id| !excluded.contains(&id))
            .is_some_and(|hit| {
                (hit.position - origin).dot(direction) < depth - depth.abs().max(1.) * 1e-4
            });
        if !occluded {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::shell::overlay;
    use glam::Mat4;
    #[test]
    fn vertex_query_uses_host_local_orthographic_geometry_and_excludes_the_selected_source() {
        let scene =
            super::super::tests::scene(&[Mat4::from_translation(-Vec3::Z * 20.), Mat4::IDENTITY]);
        let camera = Camera {
            target: Vec3::ZERO,
            yaw: 0.,
            pitch: 0.,
            distance: 5.,
            projection: crate::state::CameraProjection::Orthographic,
            orthographic_height: 4.,
            ..Default::default()
        };
        let rect = Box2 {
            x: 200.,
            y: 120.,
            w: 400.,
            h: 400.,
        };
        let point = overlay::project(&camera, rect, Vec3::new(-1., -1., 0.)).unwrap();
        let front =
            nearest_vertex(&scene, &camera, rect, point, 18., &BTreeSet::new(), None).unwrap();
        assert_eq!(front.entity, 1);
        assert_eq!(front.position, Vec3::new(-1., -1., 0.));
        let behind = nearest_vertex(
            &scene,
            &camera,
            rect,
            point,
            18.,
            &BTreeSet::from([1]),
            None,
        )
        .unwrap();
        assert_eq!(behind.entity, 0);
        assert_eq!(behind.position, Vec3::new(-1., -1., -20.));
        assert!(
            nearest_vertex(
                &scene,
                &camera,
                rect,
                point,
                18.,
                &BTreeSet::new(),
                Some(&BTreeSet::from([0]))
            )
            .is_none(),
            "source vertex behind another object is not reachable"
        );
        assert!(
            nearest_vertex(
                &scene,
                &camera,
                rect,
                [f32::NAN, 0.],
                18.,
                &BTreeSet::new(),
                None
            )
            .is_none()
        );
        assert!(
            nearest_vertex(
                &scene,
                &camera,
                rect,
                [rect.x, rect.y],
                18.,
                &BTreeSet::new(),
                None
            )
            .is_none()
        );
    }
    #[test]
    fn source_vertices_are_real_referenced_geometry_with_transformed_normals() {
        let scene = super::super::tests::scene(&[Mat4::from_scale_rotation_translation(
            Vec3::new(-2., 3., 0.5),
            glam::Quat::IDENTITY,
            Vec3::X * 4.,
        )]);
        let source = vertices(&scene, |id| id == 0);
        assert_eq!(source.len(), 3);
        assert_eq!(source[0].position, Vec3::new(6., -3., 0.));
        assert_eq!(source[0].normal, Vec3::Z);
        assert!(vertices(&scene, |_| false).is_empty());
    }
}
