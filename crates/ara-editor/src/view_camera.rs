//! The Scene view camera a scene opens with, and what framing looks at.
//! - Opening: the last camera used in that scene ([`SavedCameras`], a user
//!   preference), else the scene's gameplay camera ([`gameplay`]), else its content
//!   from a three-quarter view ([`opening`]).
//! - Framing (F, Frame All, first opens) covers renderable content: terrain, ground
//!   planes and unbounded entities (sky, lights, audio, colliders) count only when
//!   selected ([`framing_bounds`]).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_doc::prefab::{Resolved, ResolvedEntity};
use glam::{Mat4, Vec3};
use serde_json::{Value, json};

use crate::state::{Camera, EditorState};

/// A world-space axis-aligned box: (min, max).
type Bounds = (Vec3, Vec3);

/// How far in front of a collider a follow rig's shortened boom stops, in metres.
const BOOM_MARGIN: f32 = 0.25;
/// The shortest boom a collider leaves a follow rig, in metres.
const MIN_BOOM: f32 = 0.3;

/// Components that make an entity terrain (framed only when selected).
const TERRAIN: &[&str] = &["Terrain", "TerrainChunk", "Landscape"];

/// The camera scene content opens with when it has no saved camera: its gameplay
/// camera, else its content from a three-quarter view.
pub(crate) fn opening(resolved: &Resolved, transforms: &BTreeMap<String, Mat4>) -> Camera {
    gameplay(resolved, transforms).unwrap_or_else(|| {
        let mut camera = Camera {
            yaw: 0.6,
            pitch: 0.45,
            ..Camera::default()
        };
        if let Some(bounds) = framing_bounds(resolved, transforms, &[]) {
            fit(&mut camera, bounds);
        }
        camera
    })
}

/// The view of the scene's gameplay camera (the lowest `Camera.priority`, first by
/// address on ties) with its field of view. A follow rig looks from behind its target
/// and pivots on the aim point, where Play places it (`ara_runtime` `place_cameras`).
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
        let (yaw, pitch) = (rig.yaw.to_radians(), rig.pitch.to_radians());
        let forward = Vec3::new(
            yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        let aim = target.w_axis.truncate() + Vec3::Y * rig.height;
        let distance = rig.distance.max(0.05);
        // The boom stops short of solid colliders, like a camera arm: a target by a
        // wall is seen from inside the room, not through the wall.
        let reach = boom(resolved, transforms, aim, -forward, distance);
        camera.distance = if reach < distance {
            (reach - BOOM_MARGIN).max(MIN_BOOM)
        } else {
            distance
        };
        camera.look_from(aim - forward * camera.distance, forward);
    } else {
        let (_, rotation, position) = transforms.get(address)?.to_scale_rotation_translation();
        camera.look_from(position, rotation * Vec3::NEG_Z);
    }
    Some(camera)
}

/// How far from `origin` along unit `direction` the first solid box `Collider`
/// starts, within `max` (`max` when none does). Boxes containing `origin` and
/// sensors are ignored.
fn boom(
    resolved: &Resolved,
    transforms: &BTreeMap<String, Mat4>,
    origin: Vec3,
    direction: Vec3,
    max: f32,
) -> f32 {
    let mut reach = max;
    for (id, entity) in resolved {
        let Some(collider) = entity.record.get("Collider") else {
            continue;
        };
        let solid_box = collider
            .get("kind")
            .and_then(Value::as_str)
            .is_none_or(|k| k == "box")
            && collider.get("sensor").and_then(Value::as_bool) != Some(true);
        let Some(world) = transforms.get(id).filter(|_| solid_box) else {
            continue;
        };
        let half = crate::world::vector(collider.get("halfExtents"), Vec3::splat(0.5));
        let centre = crate::world::vector(collider.get("center"), Vec3::ZERO);
        // In the box's own space the ray parameter is unchanged (an affine map).
        let local = world.inverse();
        let from = local.transform_point3(origin) - centre;
        let along = local.transform_vector3(direction);
        let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
        for axis in 0..3 {
            if along[axis].abs() < 1e-8 {
                if from[axis].abs() > half[axis] {
                    far = f32::NEG_INFINITY;
                }
                continue;
            }
            let a = (-half[axis] - from[axis]) / along[axis];
            let b = (half[axis] - from[axis]) / along[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
        if near > 0. && near <= far && near < reach {
            reach = near;
        }
    }
    reach
}

/// World bounds framing shows: the `selection` when there is one (anything selected
/// counts), else renderable content without terrain, ground planes and unbounded
/// entities, else every entity. An entity is its transformed unit box (procedural
/// shapes use their `size`); `None` for an empty scene.
pub(crate) fn framing_bounds(
    resolved: &Resolved,
    transforms: &BTreeMap<String, Mat4>,
    selection: &[String],
) -> Option<(Vec3, Vec3)> {
    let boxes = |keep: &dyn Fn(&str, &ResolvedEntity, Bounds) -> bool| {
        resolved
            .iter()
            .filter_map(|(id, entity)| {
                let bounds = world_box(entity, transforms.get(id)?);
                keep(id, entity, bounds).then_some(bounds)
            })
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    };
    if !selection.is_empty() {
        return boxes(&|id, _, _| selection.iter().any(|s| s == id));
    }
    boxes(&|_, entity, bounds| content(entity, bounds)).or_else(|| boxes(&|_, _, _| true))
}

/// Aim `camera` at the bounds' centre from its current direction, far enough that a
/// sphere around them fills the view's height.
pub(crate) fn fit(camera: &mut Camera, (min, max): (Vec3, Vec3)) {
    let radius = ((max - min).length() * 0.5).max(0.5);
    let half = (camera.fov_y_degrees.to_radians() * 0.5).sin();
    camera.target = (min + max) * 0.5;
    camera.distance = (radius / half * 1.1).clamp(0.05, 100_000.);
}

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

/// An entity's world box: its unit box, or its procedural shape's `size` (planes are
/// flat), through `matrix`.
fn world_box(entity: &ResolvedEntity, matrix: &Mat4) -> (Vec3, Vec3) {
    let mut extent = Vec3::ONE;
    if let Some(renderer) = entity.record.get("MeshRenderer")
        && renderer.get("model").is_none()
    {
        if let Some(size) = renderer.get("size").and_then(Value::as_array) {
            let axis = |i: usize| size.get(i).and_then(Value::as_f64).unwrap_or(1.) as f32;
            extent = Vec3::new(axis(0), axis(1), axis(2));
        }
        if renderer.get("shape").and_then(Value::as_str) == Some("plane") {
            extent.y = 0.;
        }
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for x in [-0.5, 0.5] {
        for y in [-0.5, 0.5] {
            for z in [-0.5, 0.5] {
                let p = matrix.transform_point3(Vec3::new(x, y, z) * extent);
                min = min.min(p);
                max = max.max(p);
            }
        }
    }
    (min, max)
}

/// The Scene view's last camera per scene, kept per project in `scene-cameras.json`
/// beside the preferences file (none: memory only, as in tests). Changes are written
/// at most once a second and when the editor closes.
pub(crate) struct SavedCameras {
    path: Option<PathBuf>,
    /// This project's key in the file: its absolute root.
    project: String,
    /// Scene document → last camera, as last written or loaded.
    pub saved: BTreeMap<String, Camera>,
    dirty: bool,
    written: Option<Instant>,
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
        let saved = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|file| serde_json::from_value(file["projects"][&project].clone()).ok())
            .unwrap_or_default();
        Self {
            path,
            project,
            saved,
            dirty: false,
            written: None,
        }
    }

    /// Remember `state`'s cameras (the open scene's and those it kept for others);
    /// writes when changed, at most once a second unless `now` (closing).
    pub fn observe(&mut self, state: &EditorState, now: bool) {
        for (doc, camera) in state
            .scene_cameras
            .iter()
            .chain([(&state.doc, &state.camera)])
        {
            if self.saved.get(doc) != Some(camera) {
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
        let Some(path) = &self.path else {
            return;
        };
        let mut file = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        file["projects"][&self.project] = json!(self.saved);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Best effort: a lost camera only means the scene opens framed next time.
        let _ = crate::settings::write_atomic(path, &file);
    }
}
