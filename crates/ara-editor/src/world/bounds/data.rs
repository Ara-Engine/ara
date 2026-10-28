use super::Aabb;
use ara_runtime::model::Model;
use glam::Vec3;
use serde_json::Value;
use std::{collections::BTreeMap, ops::Range};
struct Node {
    range: Range<usize>,
    offset: Vec3,
}
pub(super) struct LocalModel {
    primitives: Vec<Option<Aabb>>,
    nodes: BTreeMap<u32, Node>,
    names: BTreeMap<String, Option<u32>>,
}
impl LocalModel {
    pub(super) fn read(model: Model) -> Self {
        let primitives = model
            .primitives
            .iter()
            .map(|primitive| {
                primitive
                    .vertices
                    .iter()
                    .filter_map(|vertex| Aabb::new(vertex.position.into(), vertex.position.into()))
                    .reduce(Aabb::union)
            })
            .collect();
        let nodes = model
            .nodes
            .into_iter()
            .map(|(id, node)| {
                (
                    id,
                    Node {
                        range: node.primitives,
                        offset: -Vec3::from(node.origin),
                    },
                )
            })
            .collect();
        Self {
            primitives,
            nodes,
            names: model.node_names,
        }
    }
    pub(super) fn selected(&self, renderer: &Value) -> Option<Aabb> {
        let (mut range, offset) =
            if let Some(selector) = renderer.get("node").and_then(Value::as_str) {
                let id = if let Some(name) = selector.strip_prefix("name:") {
                    *self.names.get(name)?.as_ref()?
                } else if let Ok(id) = selector.parse::<u32>() {
                    id
                } else {
                    *self.names.get(selector)?.as_ref()?
                };
                let node = self.nodes.get(&id)?;
                (node.range.clone(), node.offset)
            } else {
                (0..self.primitives.len(), Vec3::ZERO)
            };
        if let Some(index) = renderer.get("primitive").and_then(Value::as_u64) {
            let index = usize::try_from(index).ok()?;
            let selected = range.start.checked_add(index)?;
            if !range.contains(&selected) {
                return None;
            }
            range = selected..selected.checked_add(1)?;
        }
        let bounds = self
            .primitives
            .get(range)?
            .iter()
            .copied()
            .flatten()
            .reduce(Aabb::union)?;
        Aabb::new(bounds.min + offset, bounds.max + offset)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ara_runtime::model::{ModelNode, ModelVertex, Primitive};
    #[test]
    fn imported_vertices_and_selected_node_pivot_define_bounds_instead_of_a_unit_cube() {
        let vertices = |points: Vec<[f32; 3]>| {
            points
                .into_iter()
                .map(|position| ModelVertex {
                    position,
                    ..ModelVertex::default()
                })
                .collect()
        };
        let model = Model {
            asset: "as_bounds01".into(),
            digest: ara_asset::Digest([1; 32]),
            primitives: vec![
                Primitive {
                    vertices: vertices(vec![[-100., -1., -2.], [100., 1., 2.]]),
                    indices: Vec::new(),
                    lods: Vec::new(),
                    material: None,
                    skin: None,
                },
                Primitive {
                    vertices: vertices(vec![[12., 3., 20.], [32., 7., 24.]]),
                    indices: Vec::new(),
                    lods: Vec::new(),
                    material: None,
                    skin: None,
                },
            ],
            nodes: BTreeMap::from([(
                3,
                ModelNode {
                    index: 3,
                    name: "Long beam".into(),
                    origin: [12., 3., 20.],
                    primitives: 1..2,
                },
            )]),
            node_names: BTreeMap::from([("Long beam".into(), Some(3))]),
            materials: Vec::new(),
            textures: Vec::new(),
            skins: Vec::new(),
            bounds: [[-100., -1., -2.], [100., 7., 24.]],
        };
        let bounds = LocalModel::read(model);
        assert_eq!(
            bounds.selected(&serde_json::json!({})).unwrap(),
            Aabb {
                min: Vec3::new(-100., -1., -2.),
                max: Vec3::new(100., 7., 24.)
            }
        );
        assert_eq!(
            bounds
                .selected(&serde_json::json!({"node":"Long beam"}))
                .unwrap(),
            Aabb {
                min: Vec3::ZERO,
                max: Vec3::new(20., 4., 4.)
            }
        );
        assert!(
            bounds
                .selected(&serde_json::json!({"node":"Long beam","primitive":1}))
                .is_none()
        );
    }
}
