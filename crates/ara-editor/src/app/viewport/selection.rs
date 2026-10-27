//! Marquee selection borrows retained native instance bounds; it never reads files or GPU pixels.
use super::SceneView;
use crate::world::channels::Channels;
use ara_render::instanced::Scene;
use ara_ui::paint::Box2;
use glam::{Mat4, Vec3, Vec4};
use std::collections::BTreeSet;

impl SceneView {
    /// Actual world-space bounds of the selected rendered primitives, for framing evidence.
    pub(in crate::app) fn selected_bounds(
        &self,
        selection: &[String],
        channels: &Channels,
    ) -> Option<[Vec3; 2]> {
        let scene = self
            .raster
            .as_ref()
            .and_then(ara_runtime_render::RasterView::scene)
            .or_else(|| self.prepared.as_ref().map(|prepared| &prepared.scene))?;
        scene
            .instances
            .iter()
            .filter_map(|instance| {
                let address = channels.target_of(instance.entity)?;
                if !selection.iter().any(|selected| selected == address) {
                    return None;
                }
                let bounds = instance.bounds(scene.geometry.get(instance.geometry)?);
                (bounds.iter().all(|point| point.is_finite()) && bounds[0].cmple(bounds[1]).all())
                    .then_some(bounds)
            })
            .reduce(|a, b| [a[0].min(b[0]), a[1].max(b[1])])
    }
    /// Select unique scene addresses whose projected instance bounds touch a marquee.
    /// `contained` requires the entire visible projection inside the rectangle.
    #[allow(
        clippy::too_many_arguments,
        reason = "camera, retained identity map and selection filters are separate owners"
    )]
    pub(in crate::app) fn marquee(
        &self,
        camera: &crate::state::Camera,
        viewport: Box2,
        rect: Box2,
        channels: &Channels,
        hidden: &BTreeSet<String>,
        locked: &BTreeSet<String>,
        contained: bool,
    ) -> Vec<String> {
        let Some(scene) = self
            .raster
            .as_ref()
            .and_then(ara_runtime_render::RasterView::scene)
            .or_else(|| self.prepared.as_ref().map(|prepared| &prepared.scene))
        else {
            return Vec::new();
        };
        if !valid_rect(viewport) || !valid_rect(rect) {
            return Vec::new();
        }
        let Ok(camera) = camera
            .spec()
            .resolve([viewport.w.max(1.) as u32, viewport.h.max(1.) as u32])
        else {
            return Vec::new();
        };
        select(
            scene,
            camera.projection * camera.view,
            viewport,
            rect,
            channels,
            hidden,
            locked,
            contained,
        )
    }
}

/// Normalize a press/move pair and clip its drag rectangle to the Scene viewport.
pub(in crate::app) fn marquee_rect(from: [f32; 2], to: [f32; 2], viewport: Box2) -> Option<Box2> {
    if !from.into_iter().chain(to).all(f32::is_finite) || !valid_rect(viewport) {
        return None;
    }
    let rect = Box2 {
        x: from[0].min(to[0]),
        y: from[1].min(to[1]),
        w: (to[0] - from[0]).abs(),
        h: (to[1] - from[1]).abs(),
    }
    .intersection(viewport);
    valid_rect(rect).then_some(rect)
}

#[allow(
    clippy::too_many_arguments,
    reason = "pure projection seam mirrors native selection inputs"
)]
fn select(
    scene: &Scene,
    view_projection: Mat4,
    viewport: Box2,
    rect: Box2,
    channels: &Channels,
    hidden: &BTreeSet<String>,
    locked: &BTreeSet<String>,
    contained: bool,
) -> Vec<String> {
    if !view_projection.is_finite() {
        return Vec::new();
    }
    let mut hits = BTreeSet::new();
    for instance in &scene.instances {
        let Some(address) = channels.target_of(instance.entity) else {
            continue;
        };
        if hidden.contains(address) || locked.contains(address) {
            continue;
        }
        let Some(geometry) = scene.geometry.get(instance.geometry) else {
            continue;
        };
        let Some(bounds) = projected_bounds(instance.bounds(geometry), view_projection, viewport)
        else {
            continue;
        };
        let selected = if contained {
            bounds.x >= rect.x
                && bounds.y >= rect.y
                && bounds.x + bounds.w <= rect.x + rect.w
                && bounds.y + bounds.h <= rect.y + rect.h
        } else {
            bounds.intersects(rect)
        };
        if selected {
            hits.insert(address);
        }
    }
    hits.into_iter().map(str::to_owned).collect()
}

/// Clip AABB edges at the near plane before projecting, so geometry behind the eye
/// cannot invert a box or select an unrelated screen region.
fn projected_bounds(bounds: [Vec3; 2], view_projection: Mat4, viewport: Box2) -> Option<Box2> {
    if !bounds.iter().all(|point| point.is_finite()) || !bounds[0].cmple(bounds[1]).all() {
        return None;
    }
    let corners: [Vec4; 8] = std::array::from_fn(|index| {
        view_projection
            * Vec3::new(
                if index & 1 == 0 {
                    bounds[0].x
                } else {
                    bounds[1].x
                },
                if index & 2 == 0 {
                    bounds[0].y
                } else {
                    bounds[1].y
                },
                if index & 4 == 0 {
                    bounds[0].z
                } else {
                    bounds[1].z
                },
            )
            .extend(1.)
    });
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut include = |point: Vec4| {
        if !point.is_finite() || point.w <= f32::EPSILON {
            return;
        }
        let ndc = point.truncate() / point.w;
        let p = [
            viewport.x + (ndc.x + 1.) * 0.5 * viewport.w,
            viewport.y + (1. - ndc.y) * 0.5 * viewport.h,
        ];
        if !p.into_iter().all(f32::is_finite) {
            return;
        }
        for axis in 0..2 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    };
    for point in corners {
        if point.w - point.z >= 0. && point.z >= 0. {
            include(point);
        }
    }
    for start in 0..8 {
        for axis in [1, 2, 4] {
            let end = start ^ axis;
            if end <= start {
                continue;
            }
            let (a, b) = (corners[start], corners[end]);
            let (da, db) = (a.w - a.z, b.w - b.z);
            if (da >= 0.) != (db >= 0.) {
                let t = da / (da - db);
                if t.is_finite() {
                    include(a + (b - a) * t);
                }
            }
        }
    }
    let rect = Box2 {
        x: min[0],
        y: min[1],
        w: max[0] - min[0],
        h: max[1] - min[1],
    }
    .intersection(viewport);
    valid_rect(rect).then_some(rect)
}
fn valid_rect(rect: Box2) -> bool {
    [rect.x, rect.y, rect.w, rect.h]
        .into_iter()
        .all(f32::is_finite)
        && rect.w > 0.
        && rect.h > 0.
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ara_doc::{
        AssetId, RecordId,
        prefab::{Resolved, ResolvedEntity},
    };
    use ara_render::instanced::{Geometry, Instance};
    fn fixture() -> (Scene, Channels, Mat4, Box2) {
        let mut entities = Resolved::new();
        entities.insert(
            "e_box001".into(),
            ResolvedEntity {
                record: serde_json::json!({"MeshRenderer":{}})
                    .as_object()
                    .unwrap()
                    .clone(),
                parent: None,
                order: None,
                source: (
                    AssetId::parse("sc_test001").unwrap(),
                    RecordId::parse("e_box001").unwrap(),
                ),
            },
        );
        let id = crate::world::render_id("e_box001");
        let mut channels = Channels::default();
        channels.update(
            &entities,
            1,
            &[(id, "Box part 1".into()), (id + 1, "Box part 2".into())],
        );
        let scene = Scene {
            geometry: vec![Geometry {
                key: [1; 32],
                vertices: Vec::new(),
                lods: Vec::new(),
                bounds: [Vec3::splat(-0.5), Vec3::splat(0.5)],
                deforming: false,
                revision: 0,
            }],
            instances: [id, id + 1]
                .into_iter()
                .map(|entity| Instance {
                    entity,
                    name: "Box".into(),
                    geometry: 0,
                    material: 0,
                    world: Mat4::from_translation(Vec3::new(0., 0., -5.)),
                    previous_world: Mat4::IDENTITY,
                })
                .collect(),
            materials: Vec::new(),
            textures: 0,
            lights: Vec::new(),
            sun_direction: Vec3::Y,
            sun_lux: Vec3::ONE,
            environment: Vec::new(),
            environment_size: [0; 2],
        };
        let camera = ara_render::scene::CameraSpec::LookAt {
            position: Vec3::ZERO,
            target: -Vec3::Z,
            vertical_fov: 60_f32.to_radians(),
        }
        .resolve([800, 600])
        .unwrap();
        (
            scene,
            channels,
            camera.projection * camera.view,
            Box2 {
                x: 0.,
                y: 0.,
                w: 800.,
                h: 600.,
            },
        )
    }
    #[test]
    fn marquee_deduplicates_model_parts_and_excludes_hidden_locked_and_outside_entities() {
        let (scene, channels, vp, viewport) = fixture();
        let empty = BTreeSet::new();
        let rect = Box2 {
            x: 300.,
            y: 200.,
            w: 200.,
            h: 200.,
        };
        assert_eq!(
            select(&scene, vp, viewport, rect, &channels, &empty, &empty, false),
            ["e_box001"]
        );
        let hidden = BTreeSet::from(["e_box001".into()]);
        assert!(
            select(
                &scene, vp, viewport, rect, &channels, &hidden, &empty, false
            )
            .is_empty()
        );
        assert!(
            select(
                &scene, vp, viewport, rect, &channels, &empty, &hidden, false
            )
            .is_empty()
        );
        assert!(
            select(
                &scene,
                vp,
                viewport,
                Box2 {
                    x: 0.,
                    y: 0.,
                    w: 40.,
                    h: 40.
                },
                &channels,
                &empty,
                &empty,
                false
            )
            .is_empty()
        );
    }
    #[test]
    fn reverse_drags_containment_and_near_plane_crossing_have_finite_bounds() {
        let (mut scene, channels, vp, viewport) = fixture();
        let empty = BTreeSet::new();
        let rect = marquee_rect([500., 400.], [300., 200.], viewport).unwrap();
        assert_eq!(
            select(&scene, vp, viewport, rect, &channels, &empty, &empty, true),
            ["e_box001"]
        );
        scene.instances[0].world = Mat4::from_translation(Vec3::new(0., 0., 0.1));
        assert!(
            projected_bounds(scene.instances[0].bounds(&scene.geometry[0]), vp, viewport).is_some()
        );
        scene.instances[0].world = Mat4::from_translation(Vec3::new(0., 0., 5.));
        assert!(
            projected_bounds(scene.instances[0].bounds(&scene.geometry[0]), vp, viewport).is_none()
        );
        assert!(marquee_rect([f32::NAN, 0.], [2., 2.], viewport).is_none());
    }
}
