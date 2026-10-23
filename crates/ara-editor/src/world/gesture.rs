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
    local: bool,
    snap: Option<f32>,
}
impl Gesture {
    /// Capture all selected transforms once. Selected descendants follow their selected ancestor.
    pub fn begin(entities: &Resolved, state: &EditorState, base: u64) -> anyhow::Result<Self> {
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
        let snap = state.snapping.then_some(match state.tool {
            Tool::Translate => state.move_snap,
            Tool::Rotate => state.rotate_snap,
            Tool::Scale => state.scale_snap,
        });
        Ok(Self {
            base,
            tool: state.tool,
            current: initial.clone(),
            initial,
            parents,
            pivot,
            centre: state.centre,
            local: state.local,
            snap,
        })
    }
    /// Update the provisional delta from the original pose, never from the preceding sample.
    pub fn update(&mut self, delta: Vec3) {
        let delta = if let Some(snap) = self.snap {
            delta.map(|v| (v / snap).round() * snap)
        } else {
            delta
        };
        for (id, start) in &self.initial {
            let parent = self.parents[id];
            let (scale, rotation, position) = start.to_scale_rotation_translation();
            let world = parent * (*start);
            let (_, world_rotation, world_position) = world.to_scale_rotation_translation();
            let next = match self.tool {
                Tool::Translate => {
                    let movement = if self.local {
                        world_rotation * delta
                    } else {
                        delta
                    };
                    let p = parent.inverse().transform_point3(world_position + movement);
                    Mat4::from_scale_rotation_translation(scale, rotation, p)
                }
                Tool::Rotate => {
                    let d = delta.map(f32::to_radians);
                    let q = Quat::from_euler(EulerRot::XYZ, d.x, d.y, d.z);
                    if self.local && !self.centre {
                        Mat4::from_scale_rotation_translation(scale, rotation * q, position)
                    } else {
                        let pivot = if self.centre {
                            self.pivot
                        } else {
                            world_position
                        };
                        parent.inverse()
                            * Mat4::from_translation(pivot)
                            * Mat4::from_quat(q)
                            * Mat4::from_translation(-pivot)
                            * world
                    }
                }
                Tool::Scale => {
                    let factor = (Vec3::ONE + delta).max(Vec3::splat(0.001));
                    let p = if self.centre {
                        parent
                            .inverse()
                            .transform_point3(self.pivot + (world_position - self.pivot) * factor)
                    } else {
                        position
                    };
                    Mat4::from_scale_rotation_translation(scale * factor, rotation, p)
                }
            };
            self.current.insert(id.clone(), next);
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
