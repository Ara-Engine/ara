//! The Scene view camera a scene opens with, and what framing looks at.
//! - Opening: the last camera used in that scene ([`SavedCameras`], a user
//!   preference), else the scene's gameplay camera ([`gameplay`]), else its content
//!   from a three-quarter view ([`opening_with_models`]).
//! - Framing (F, Frame All, first opens) covers renderable content: terrain, ground
//!   planes and unbounded entities (sky, lights, audio, colliders) count only when
//!   selected ([`framing_bounds_with_models`]). Imported geometry uses published bounds.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use ara_doc::prefab::{Resolved, ResolvedEntity};
use ara_physics::{Collider, RigidBody};
use ara_runtime::follow::{FollowView, Wall};
use glam::{Mat4, Vec3};
use serde_json::{Value, json};

use crate::state::{Camera, EditorState};

/// A world-space axis-aligned box: (min, max).
type Bounds = (Vec3, Vec3);

/// Components that make an entity terrain (framed only when selected).
const TERRAIN: &[&str] = &["Terrain", "TerrainChunk", "Landscape"];

/// The view of the scene's gameplay camera (the lowest `Camera.priority`, first by
/// address on ties) with its field of view. A follow rig looks from behind its target,
/// its boom stopped by walls, and pivots on the aim point: where Play places it
/// (`ara_runtime::follow::FollowView`).
pub(crate) fn gameplay(resolved: &Resolved, transforms: &BTreeMap<String, Mat4>) -> Option<Camera> {
    use ara_runtime::components::{Camera as Lens, FollowCamera};
    let (address, lens, entity) = resolved
        .iter()
        .filter_map(|(address, entity)| {
            let lens: Lens = serde_json::from_value(entity.record.get("Camera")?.clone()).ok()?;
            Some((address, lens, entity))
        })
        .min_by_key(|(_, lens, _)| lens.priority)?;
    let mut camera = Camera::default();
    camera.set_fov(lens.fov);
    let rig = entity
        .record
        .get("FollowCamera")
        .and_then(|v| serde_json::from_value::<FollowCamera>(v.clone()).ok());
    if let Some(rig) = rig
        && let Some(target) = rig
            .target
            .as_ref()
            .and_then(|t| transforms.get(t.0.as_str()))
    {
        // The runtime is on a newer glam: values cross as arrays.
        let target = target.w_axis.truncate().to_array().into();
        let view = FollowView::new(&rig, target, walls(resolved, transforms));
        let (aim, forward) = (
            Vec3::from(view.aim.to_array()),
            Vec3::from(view.forward.to_array()),
        );
        camera.distance = view.distance.max(0.05);
        camera.look_from(aim - forward * camera.distance, forward);
    } else {
        let (_, rotation, position) = transforms.get(address)?.to_scale_rotation_translation();
        camera.look_from(position, rotation * Vec3::NEG_Z);
    }
    Some(camera)
}

/// The walls a follow rig's boom stops in front of, as Play finds them: static, solid
/// box colliders at their world pose ([`Wall::solid`]).
fn walls<'a>(
    resolved: &'a Resolved,
    transforms: &'a BTreeMap<String, Mat4>,
) -> impl Iterator<Item = Wall> + 'a {
    resolved.iter().filter_map(|(id, entity)| {
        let collider: Collider =
            serde_json::from_value(entity.record.get("Collider")?.clone()).ok()?;
        let body: RigidBody =
            serde_json::from_value(entity.record.get("RigidBody")?.clone()).ok()?;
        if !Wall::solid(&collider, &body) {
            return None;
        }
        let (_, rotation, translation) = transforms.get(id)?.to_scale_rotation_translation();
        Some(Wall::new(
            &collider,
            translation.to_array().into(),
            glam34::Quat::from_array(rotation.to_array()),
        ))
    })
}

/// Frame actual imported/procedural bounds. Selected parents include their renderable
/// descendants; pending imports keep the camera's last view until metadata is available.
pub(crate) fn framing_bounds_with_models(
    resolved: &Resolved,
    transforms: &BTreeMap<String, Mat4>,
    selection: &[String],
    models: &crate::world::bounds::ModelBounds,
) -> Option<Bounds> {
    use crate::world::bounds::{Aabb, selected};
    let world = |entity: &ResolvedEntity, matrix: Mat4| {
        if let Some(renderer) = entity.record.get("MeshRenderer") {
            if renderer.get("model").and_then(Value::as_str).is_some() {
                return models.local(renderer)?.transformed(matrix);
            }
            let size = renderer.get("size").and_then(Value::as_array);
            let axis = |index| {
                size.and_then(|size| size.get(index))
                    .and_then(Value::as_f64)
                    .unwrap_or(1.) as f32
            };
            let mut half = Vec3::new(axis(0), axis(1), axis(2)) * 0.5;
            if renderer.get("shape").and_then(Value::as_str) == Some("plane") {
                half.y = 0.;
            }
            return Aabb::new(-half, half)?.transformed(matrix);
        }
        Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5))?.transformed(matrix)
    };
    let boxes = |renderable: bool| {
        resolved
            .iter()
            .filter_map(|(id, entity)| {
                if !selection.is_empty() && !selected(resolved, id, selection) {
                    return None;
                }
                if renderable && !entity.record.contains_key("MeshRenderer") {
                    return None;
                }
                let bounds = world(entity, *transforms.get(id)?)?;
                if selection.is_empty() && renderable && !content(entity, (bounds.min, bounds.max))
                {
                    return None;
                }
                Some(bounds)
            })
            .reduce(Aabb::union)
    };
    let bounds = boxes(true).or_else(|| boxes(false))?;
    Some((bounds.min, bounds.max))
}

/// Fit all eight bound corners against both field-of-view axes. Long, thin and
/// portrait viewports therefore retain every selected corner without changing view direction.
pub(crate) fn fit_aspect(camera: &mut Camera, (min, max): Bounds, aspect: f32) {
    if !min.is_finite()
        || !max.is_finite()
        || !min.cmple(max).all()
        || !aspect.is_finite()
        || aspect <= 0.
    {
        return;
    }
    let centre = min * 0.5 + max * 0.5;
    let Some((right, up, forward)) = camera.basis() else {
        return;
    };
    let back = -forward;
    if !back.is_finite() || !right.is_finite() || !up.is_finite() || !centre.is_finite() {
        return;
    }
    let tan_y = (f64::from(camera.fov_y_degrees) * std::f64::consts::PI / 360.).tan();
    let tan_x = tan_y * f64::from(aspect);
    if !tan_y.is_finite() || !tan_x.is_finite() || tan_y <= 0. || tan_x <= 0. {
        return;
    }
    let mut distance = 0.5_f64;
    let mut vertical_span = 0.01_f64;
    for index in 0..8 {
        let corner = Vec3::new(
            if index & 1 == 0 { min.x } else { max.x },
            if index & 2 == 0 { min.y } else { max.y },
            if index & 4 == 0 { min.z } else { max.z },
        );
        let offset = corner.as_dvec3() - centre.as_dvec3();
        let depth = offset.dot(back.as_dvec3());
        vertical_span = vertical_span
            .max(2.2 * offset.dot(up.as_dvec3()).abs())
            .max(2.2 * offset.dot(right.as_dvec3()).abs() / f64::from(aspect));
        distance = distance
            .max(
                depth
                    + (offset.dot(right.as_dvec3()).abs() * 1.1 / tan_x)
                        .max(offset.dot(up.as_dvec3()).abs() * 1.1 / tan_y),
            )
            .max(depth + 0.11);
    }
    if distance.is_finite() {
        camera.target = centre;
        camera.distance = distance.clamp(0.05, 100_000.) as f32;
        if camera.projection == crate::state::CameraProjection::Orthographic {
            camera.orthographic_height = vertical_span.clamp(0.01, 1_000_000.) as f32;
        }
    }
}

/// The first view of a scene, using its gameplay camera or published content bounds.
pub(crate) fn opening_with_models(
    resolved: &Resolved,
    transforms: &BTreeMap<String, Mat4>,
    models: &crate::world::bounds::ModelBounds,
    aspect: f32,
) -> Camera {
    gameplay(resolved, transforms).unwrap_or_else(|| {
        let mut camera = Camera {
            yaw: 0.6,
            pitch: 0.45,
            ..Camera::default()
        };
        if let Some(bounds) = framing_bounds_with_models(resolved, transforms, &[], models) {
            fit_aspect(&mut camera, bounds, aspect);
        }
        camera
    })
}

#[cfg(test)]
#[path = "view_camera/framing_tests.rs"]
mod framing_tests;

/// Renderable, bounded and not ground: a `MeshRenderer` that is not terrain and not
/// a wide flat slab (20 m or more across, under 5% as tall).
fn content(entity: &ResolvedEntity, (min, max): (Vec3, Vec3)) -> bool {
    let record = &entity.record;
    if !record.contains_key("MeshRenderer") || TERRAIN.iter().any(|c| record.contains_key(*c)) {
        return false;
    }
    let size = max - min;
    let across = size.x.max(size.z);
    !(across >= 20. && size.y < across * 0.05)
}

/// The Scene view's last camera per scene, kept per project in `scene-cameras.json`
/// beside the preferences file (none: memory only, as in tests). Changes are written
/// at most once a second and when the editor closes, on a writer thread
/// (`crate::persist`): navigating never waits on the disk.
pub(crate) struct SavedCameras {
    /// Scene document → last camera, as last written or loaded.
    pub saved: BTreeMap<String, Camera>,
    /// Scene document to named camera views, shared with attributed navigation ops.
    pub bookmarks: BTreeMap<String, BTreeMap<String, Camera>>,
    dirty: bool,
    written: Option<Instant>,
    writer: Option<crate::persist::BackgroundWriter<SavedViews>>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SavedViews {
    cameras: BTreeMap<String, Camera>,
    bookmarks: BTreeMap<String, BTreeMap<String, Camera>>,
}

impl SavedCameras {
    /// Load the project at `root`'s saved cameras from beside `preferences`.
    pub fn open(preferences: Option<&Path>, root: &Path) -> Self {
        let path = preferences
            .and_then(Path::parent)
            .map(|dir| dir.join("scene-cameras.json"));
        let project = std::path::absolute(root)
            .unwrap_or_else(|_| root.into())
            .to_string_lossy()
            .into_owned();
        let mut views = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|file| {
                let value = file["projects"][&project].clone();
                if value.get("cameras").is_some() || value.get("bookmarks").is_some() {
                    serde_json::from_value::<SavedViews>(value).ok()
                } else {
                    serde_json::from_value(value)
                        .ok()
                        .map(|cameras| SavedViews {
                            cameras,
                            ..SavedViews::default()
                        })
                }
            })
            .unwrap_or_default();
        views.cameras.retain(|_, camera| camera.is_valid());
        for bookmarks in views.bookmarks.values_mut() {
            bookmarks.retain(|name, camera| !name.trim().is_empty() && camera.is_valid());
        }
        views.bookmarks.retain(|_, bookmarks| !bookmarks.is_empty());
        // The file holds every project's cameras: the writer re-reads it and replaces
        // only this project's entry.
        let writer = path.map(|path| {
            crate::persist::BackgroundWriter::new("cameras-writer", move |saved: SavedViews| {
                let mut file = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({}));
                file["projects"][&project] = json!(saved);
                crate::persist::write_text(&path, &ara_json::to_canonical_string(&file))
                    .map_err(|e| e.to_string())
            })
        });
        Self {
            saved: views.cameras,
            bookmarks: views.bookmarks,
            dirty: false,
            written: None,
            writer,
        }
    }

    /// Remember `state`'s cameras (the open scene's and those it kept for others);
    /// writes when changed, at most once a second unless `now` (closing).
    pub fn observe(&mut self, state: &EditorState, now: bool) {
        if self.bookmarks != state.camera_bookmarks {
            self.bookmarks.clone_from(&state.camera_bookmarks);
            self.dirty = true;
        }
        for (doc, camera) in state
            .scene_cameras
            .iter()
            .chain([(&state.doc, &state.camera)])
        {
            if camera.is_valid() && self.saved.get(doc) != Some(camera) {
                self.saved.insert(doc.clone(), *camera);
                self.dirty = true;
            }
        }
        let due = now
            || self
                .written
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
        if self.dirty && due {
            self.write();
        }
    }

    fn write(&mut self) {
        self.dirty = false;
        self.written = Some(Instant::now());
        // Best effort: a lost camera only means the scene opens framed next time.
        if let Some(writer) = &self.writer {
            writer.save(SavedViews {
                cameras: self.saved.clone(),
                bookmarks: self.bookmarks.clone(),
            });
        }
    }
}
