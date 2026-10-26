//! The Scene view camera a scene opens with, and what framing looks at.
//! - Opening: the last camera used in that scene ([`SavedCameras`], a user
//!   preference), else the scene's gameplay camera ([`gameplay`]), else its content
//!   from a three-quarter view ([`opening`]).
//! - Framing (F, Frame All, first opens) covers renderable content: terrain, ground
//!   planes and unbounded entities (sky, lights, audio, colliders) count only when
//!   selected ([`framing_bounds`]).
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
/// at most once a second and when the editor closes, on a writer thread
/// (`crate::persist`): navigating never waits on the disk.
pub(crate) struct SavedCameras {
    /// Scene document → last camera, as last written or loaded.
    pub saved: BTreeMap<String, Camera>,
    dirty: bool,
    written: Option<Instant>,
    writer: Option<crate::persist::BackgroundWriter<BTreeMap<String, Camera>>>,
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
        // The file holds every project's cameras: the writer re-reads it and replaces
        // only this project's entry.
        let writer = path.map(|path| {
            crate::persist::BackgroundWriter::new(
                "cameras-writer",
                move |saved: BTreeMap<String, Camera>| {
                    let mut file = std::fs::read(&path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({}));
                    file["projects"][&project] = json!(saved);
                    crate::persist::write_text(&path, &ara_json::to_canonical_string(&file))
                        .map_err(|e| e.to_string())
                },
            )
        });
        Self {
            saved,
            dirty: false,
            written: None,
            writer,
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
        // Best effort: a lost camera only means the scene opens framed next time.
        if let Some(writer) = &self.writer {
            writer.save(self.saved.clone());
        }
    }
}
