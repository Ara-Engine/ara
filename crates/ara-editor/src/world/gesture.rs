//! Provisional spatial edits; release creates one transaction, Escape creates none.
use crate::{
    state::{EditorState, Tool},
    world::{local_transform, vector},
};
use ara_doc::prefab::Resolved;
use glam::{EulerRot, Mat4, Quat, Vec3};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One coalesced transform gesture, including its conflict-detection base revision.
pub struct Gesture {
    /// Store revision observed at pointer-down.
    pub base: u64,
    /// Tool at the beginning of this gesture.
    pub tool: Tool,
    /// Local transform matrices before the gesture.
    pub initial: BTreeMap<String, Mat4>,
    /// Current local matrices; only render extraction sees these before commit.
    pub current: BTreeMap<String, Mat4>,
    parents: BTreeMap<String, Mat4>,
    pivot: Vec3,
    centre: bool,
    snap: Option<f32>,
    axes: Quat,
}
impl Gesture {
    /// Capture all selected transforms once. Selected descendants follow their selected ancestor.
    pub fn begin(entities: &Resolved, state: &EditorState, base: u64) -> anyhow::Result<Self> {
        Self::begin_tool(entities, state, base, state.tool)
    }
    /// Begin the operation picked from a combined gizmo, preserving the toolbar's
    /// selected tool and its coordinate, pivot and snapping settings.
    pub fn begin_tool(
        entities: &Resolved,
        state: &EditorState,
        base: u64,
        tool: Tool,
    ) -> anyhow::Result<Self> {
        let worlds = super::world_transforms(entities);
        let mut initial = BTreeMap::new();
        let mut parents = BTreeMap::new();
        let mut pivot = Vec3::ZERO;
        let mut count = 0.;
        for id in &state.selection {
            let entity = entities
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("selected entity disappeared"))?;
            if !entity.record.contains_key("Transform") {
                continue;
            }
            let mut ancestor = entity.parent.as_deref();
            let mut inherited = false;
            while let Some(parent) = ancestor {
                if state.selection.iter().any(|s| s == parent) {
                    inherited = true;
                    break;
                }
                ancestor = entities.get(parent).and_then(|e| e.parent.as_deref());
            }
            if inherited {
                continue;
            }
            let matrix = local_transform(&entity.record);
            initial.insert(id.clone(), matrix);
            parents.insert(
                id.clone(),
                entity
                    .parent
                    .as_ref()
                    .and_then(|p| worlds.get(p))
                    .copied()
                    .unwrap_or(Mat4::IDENTITY),
            );
            pivot += worlds
                .get(id)
                .copied()
                .unwrap_or(matrix)
                .transform_point3(Vec3::ZERO);
            count += 1.;
        }
        anyhow::ensure!(
            !initial.is_empty(),
            "select an entity with a Transform first"
        );
        pivot /= count;
        // The Rect tool sets whole poses (`rect_resize`); increments don't apply.
        let snap = match tool {
            Tool::Translate | Tool::Universal => Some(state.move_snap),
            Tool::Rotate => Some(state.rotate_snap),
            Tool::Scale => Some(state.scale_snap),
            Tool::Rect => None,
        }
        .filter(|_| state.snapping);
        let axes = if state.local {
            state
                .selection
                .first()
                .and_then(|id| worlds.get(id))
                .map(|m| m.to_scale_rotation_translation().1)
                .unwrap_or(Quat::IDENTITY)
        } else {
            Quat::IDENTITY
        };
        Ok(Self {
            base,
            tool,
            current: initial.clone(),
            initial,
            parents,
            pivot,
            centre: state.centre,
            snap,
            axes,
        })
    }
    /// Update the provisional delta from the original pose, never from the preceding sample.
    pub fn update(&mut self, delta: Vec3) {
        let delta = if let Some(snap) = self.snap {
            delta.map(|v| (v / snap).round() * snap)
        } else {
            delta
        };
        if delta == Vec3::ZERO {
            for (id, matrix) in &self.initial {
                if let Some(current) = self.current.get_mut(id) {
                    *current = *matrix;
                }
            }
            return;
        }
        for (id, start) in &self.initial {
            let parent = self.parents[id];
            let (scale, rotation, _) = start.to_scale_rotation_translation();
            let world = parent * (*start);
            let world_position = world.transform_point3(Vec3::ZERO);
            let next = match self.tool {
                Tool::Translate | Tool::Universal | Tool::Rect => {
                    let movement = self.axes * delta;
                    let p = parent.inverse().transform_point3(world_position + movement);
                    Mat4::from_scale_rotation_translation(scale, rotation, p)
                }
                Tool::Rotate => {
                    let d = delta.map(f32::to_radians);
                    let q = Quat::from_euler(EulerRot::XYZ, d.x, d.y, d.z);
                    {
                        let pivot = if self.centre {
                            self.pivot
                        } else {
                            world_position
                        };
                        parent.inverse()
                            * Mat4::from_translation(pivot)
                            * Mat4::from_quat(self.axes * q * self.axes.inverse())
                            * Mat4::from_translation(-pivot)
                            * world
                    }
                }
                Tool::Scale => {
                    let factor = (Vec3::ONE + delta).max(Vec3::splat(0.001));
                    let pivot = if self.centre {
                        self.pivot
                    } else {
                        world_position
                    };
                    parent.inverse()
                        * Mat4::from_translation(pivot)
                        * Mat4::from_quat(self.axes)
                        * Mat4::from_scale(factor)
                        * Mat4::from_quat(self.axes.inverse())
                        * Mat4::from_translation(-pivot)
                        * world
                }
            };
            if let Some(current) = self.current.get_mut(id) {
                *current = next;
            }
        }
    }
    /// Translate from the captured world poses, bypassing grid increments for an
    /// exact surface/vertex contact. Invalid input leaves the whole preview intact.
    pub fn translate_world(&mut self, delta: Vec3) -> bool {
        if !matches!(self.tool, Tool::Translate | Tool::Universal)
            || !delta.is_finite()
            || self.current.len() != self.initial.len()
            || self.initial.iter().any(|(id, matrix)| {
                let Some(parent) = self.parents.get(id) else {
                    return true;
                };
                !matrix.is_finite()
                    || !self.current.contains_key(id)
                    || !parent.is_finite()
                    || parent.determinant().abs() < 1e-20
                    || !(matrix.w_axis + parent.inverse().transform_vector3(delta).extend(0.))
                        .is_finite()
            })
        {
            return false;
        }
        for (id, start) in &self.initial {
            if let (Some(current), Some(parent)) = (self.current.get_mut(id), self.parents.get(id))
            {
                *current = *start;
                current.w_axis += parent.inverse().transform_vector3(delta).extend(0.);
            }
        }
        true
    }
    /// The Rect tool's resize and move of every root from its pose at pointer-down: its
    /// position goes through the world map `map`; for each rect axis in `axes` that the drag
    /// scaled by `scale` (≠ 1), only the root's own axis closest to it scales by that much,
    /// and its rotation is kept. In a rectangle drawn in the root's own frame that is
    /// exactly the rectangle's change; in a world rectangle over mixed rotations no axis the
    /// drag didn't scale grows. The preview is the exact TRS the commit writes, and an
    /// identity map leaves every pose untouched. A non-finite map changes nothing.
    pub fn rect_resize(&mut self, map: Mat4, axes: [Vec3; 2], scale: glam::Vec2) -> bool {
        if !map.is_finite() || !scale.is_finite() {
            return false;
        }
        let still = map.abs_diff_eq(Mat4::IDENTITY, 1e-6);
        for (id, start) in &self.initial {
            let (Some(current), Some(parent)) = (self.current.get_mut(id), self.parents.get(id))
            else {
                continue;
            };
            if still {
                *current = *start;
                continue;
            }
            let (size, rotation, position) = (*parent * *start).to_scale_rotation_translation();
            let own = [rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z];
            let mut stretch = Vec3::ONE;
            for (axis, by) in axes.iter().zip([scale.x, scale.y]) {
                if (by - 1.0).abs() <= 1e-6 {
                    continue;
                }
                let closest = (0..3)
                    .max_by(|&a, &b| own[a].dot(*axis).abs().total_cmp(&own[b].dot(*axis).abs()))
                    .unwrap_or(0);
                stretch[closest] *= by;
            }
            let world = Mat4::from_scale_rotation_translation(
                size * stretch,
                rotation,
                map.transform_point3(position),
            );
            // Kept as the TRS `commands` will write, so what shows is what commits.
            let (size, rotation, position) =
                (parent.inverse() * world).to_scale_rotation_translation();
            let next = Mat4::from_scale_rotation_translation(size, rotation, position);
            if next.is_finite() {
                *current = next;
            }
        }
        true
    }
    /// World-space change the preview applied to `id`'s gesture root (else the first
    /// root) since pointer-down: `after * before⁻¹`. Selected descendants move with
    /// their root, so this places the drawn gizmo at the previewed pose.
    pub fn world_delta(&self, id: Option<&str>) -> Mat4 {
        let id = id
            .filter(|id| self.initial.contains_key(*id))
            .or_else(|| self.initial.keys().next().map(String::as_str));
        let Some(id) = id else {
            return Mat4::IDENTITY;
        };
        let (Some(start), Some(current), Some(parent)) = (
            self.initial.get(id),
            self.current.get(id),
            self.parents.get(id),
        ) else {
            return Mat4::IDENTITY;
        };
        let before = *parent * *start;
        if before.determinant().abs() < 1e-12 {
            return Mat4::IDENTITY;
        }
        let delta = *parent * *current * before.inverse();
        if delta.is_finite() {
            delta
        } else {
            Mat4::IDENTITY
        }
    }
    /// One transform.set per affected entity; the caller commits this list as one cmd.apply.
    pub fn commands(&self) -> Vec<Value> {
        self.current.iter().filter(|(id,m)|self.initial.get(*id)!=Some(*m)).map(|(id,matrix)|{
            let (scale,rotation,position)=matrix.to_scale_rotation_translation();let (x,y,z)=rotation.to_euler(EulerRot::XYZ);
            json!({"cmd":"transform.set","args":{"entities":[id],"position":position.to_array(),"rotation":[x.to_degrees(),y.to_degrees(),z.to_degrees()],"scale":scale.to_array()}})
        }).collect()
    }
    pub(super) fn preview_world(
        &self,
        entities: &Resolved,
        transforms: &mut BTreeMap<String, Mat4>,
    ) {
        *transforms = super::transforms_with(entities, Some(&self.current));
    }
}
/// Read a vector field with core Transform defaults; useful for unit-entry and multi-edit.
pub fn transform_value(record: &ara_doc::Record, field: &str) -> Vec3 {
    vector(
        record.get("Transform").and_then(|t| t.get(field)),
        if field == "scale" {
            Vec3::ONE
        } else {
            Vec3::ZERO
        },
    )
}
