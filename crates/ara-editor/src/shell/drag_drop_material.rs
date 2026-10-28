//! Material drags keep every other primitive override and author one reversible field edit.
use super::*;

pub(super) fn is_material(asset: &assets::Asset) -> bool {
    matches!(asset.kind.as_str(), "material" | "materialinstance")
        || (asset.kind == "graph" && asset.domain.as_deref() == Some("material"))
}
fn picked_slot(draw: &ara_runtime::extract::Draw, renderer: &Value) -> ara_ui::Result<Option<u32>> {
    match &draw.geometry {
        ara_runtime::extract::Geometry::Shape { .. } => {
            Ok((!renderer.get("model").is_some_and(|model| !model.is_null())).then_some(0))
        }
        ara_runtime::extract::Geometry::Model { model, primitive } => {
            let model_id = renderer
                .get("model")
                .and_then(Value::as_str)
                .and_then(|reference| ara_schema::grammar::parse_ref(reference).ok())
                .and_then(|reference| reference.id);
            if model_id != Some(model.asset.as_str()) {
                return Ok(None);
            }
            let node = renderer.get("node").and_then(Value::as_str);
            let filtered = renderer
                .get("primitive")
                .and_then(Value::as_u64)
                .map(u32::try_from)
                .transpose()?;
            let selection = model.select(node, filtered)?;
            if !selection.primitives.contains(primitive) {
                return Ok(None);
            }
            Ok(Some(u32::try_from(
                primitive
                    .checked_sub(selection.base)
                    .ok_or("Invalid picked primitive slot")?,
            )?))
        }
    }
}
impl Shell {
    /// Resolve a picked native draw to its subtree-local material slot. Source geometry is borrowed.
    pub fn content_material_hit(
        &mut self,
        editor: &Editor,
        draw: &ara_runtime::extract::Draw,
    ) -> ara_ui::Result<bool> {
        let surface = surface_id(self);
        let mut drag = match self.drag.take() {
            Some(Drag::Content(drag)) => drag,
            other => {
                self.drag = other;
                return Ok(false);
            }
        };
        let result = (|| -> ara_ui::Result<bool> {
            let Payload::Asset(asset) = drag.session.payload() else {
                return Ok(false);
            };
            if !drag.session.active()
                || !is_material(asset)
                || !matches!(drag.session.feedback(), Some(Feedback::Reject { reason, .. }) if reason == NO_MATERIAL_TARGET)
            {
                return Ok(false);
            }
            let resolved = self.resolved(editor)?;
            let Some(renderer) = resolved
                .get(&draw.address)
                .and_then(|entity| entity.record.get("MeshRenderer"))
            else {
                return Ok(false);
            };
            let Some(primitive) = picked_slot(draw, renderer)? else {
                return Ok(false);
            };
            let feedback = self.material_drop_feedback(editor, asset, &draw.address, primitive)?;
            if matches!(feedback, Feedback::Accept { .. }) {
                drag.material_hit = Some(MaterialPlacement {
                    surface,
                    pointer: self.pointer,
                    entity: draw.address.to_owned(),
                    primitive,
                });
            }
            drag.session.preview(feedback);
            Ok(true)
        })();
        self.drag = Some(Drag::Content(drag));
        self.invalidate_content_drag();
        result
    }
    pub(super) fn material_drop_feedback(
        &mut self,
        editor: &Editor,
        asset: &assets::Asset,
        entity: &str,
        primitive: u32,
    ) -> ara_ui::Result<Feedback<Target>> {
        let Some(rect) = self.viewport() else {
            return Ok(reject(NO_MATERIAL_TARGET, None));
        };
        let indicator = Indicator::Inside(rect);
        if editor.state.locked.contains(entity) {
            return Ok(reject(
                "Unlock the target before assigning its material",
                Some(indicator),
            ));
        }
        let resolved = self.resolved(editor)?;
        if !resolved
            .get(entity)
            .is_some_and(|entity| entity.record.contains_key("MeshRenderer"))
        {
            return Ok(reject("The target mesh was deleted", Some(indicator)));
        }
        if asset
            .current_reference(editor.host.store().workspace())
            .is_none()
        {
            return Ok(reject("The material is not imported yet", Some(indicator)));
        }
        Ok(accept(
            Target::Material {
                entity: entity.into(),
                primitive,
            },
            indicator,
            format!("Assign material · slot {}", primitive.saturating_add(1)),
        ))
    }
    pub(super) fn commit_material_drop(
        &mut self,
        editor: &mut Editor,
        asset: &assets::Asset,
        entity: &str,
        primitive: u32,
    ) -> ara_ui::Result<()> {
        let resolved = self.resolved(editor)?;
        let renderer = resolved
            .get(entity)
            .and_then(|entity| entity.record.get("MeshRenderer"))
            .ok_or("The target mesh was deleted")?;
        let reference = asset
            .current_reference(editor.host.store().workspace())
            .ok_or("The material is no longer available")?;
        let mut overrides = renderer
            .get("materialOverrides")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let next = json!({"primitive":primitive,"material":reference});
        if let Some(existing) = overrides.iter_mut().find(|entry| {
            entry.get("primitive").and_then(Value::as_u64).unwrap_or(0) == u64::from(primitive)
        }) {
            *existing = next;
        } else {
            overrides.push(next);
        }
        overrides.sort_by_key(|entry| entry.get("primitive").and_then(Value::as_u64).unwrap_or(0));
        editor.apply(vec![json!({"cmd":"property.set","args":{"entity":entity,"path":"MeshRenderer.materialOverrides","value":overrides}})],"Assign material to picked primitive",None)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ara_runtime::{
        extract::{Draw, Geometry},
        model::{Model, ModelNode, Primitive},
    };
    use std::{collections::BTreeMap, sync::Arc};
    #[test]
    fn selected_subtree_uses_local_override_slots_and_stale_draws_are_refused() {
        let model = Arc::new(Model {
            asset: "as_model001".into(),
            digest: ara_asset::Digest([0; 32]),
            primitives: (0..3)
                .map(|_| Primitive {
                    vertices: Vec::new(),
                    indices: Vec::new(),
                    lods: Vec::new(),
                    material: None,
                    skin: None,
                })
                .collect(),
            nodes: BTreeMap::from([(
                1,
                ModelNode {
                    index: 1,
                    name: "Branch".into(),
                    origin: [0.; 3],
                    primitives: 1..3,
                },
            )]),
            node_names: BTreeMap::from([("Branch".into(), Some(1))]),
            materials: Vec::new(),
            textures: Vec::new(),
            skins: Vec::new(),
            bounds: [[0.; 3], [1.; 3]],
        });
        let draw = Draw {
            entity: 42,
            address: "e_target001".into(),
            label: "Selected branch primitive".into(),
            geometry: Geometry::Model {
                model,
                primitive: 2,
            },
            world: glam34::Mat4::IDENTITY,
            color: [1.; 4],
            roughness: 0.5,
            metallic: 0.,
            emissive: [0.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
        };
        let renderer =
            json!({"model":"@assets/model.glb#as_model001","node":"Branch","primitive":1});
        assert_eq!(picked_slot(&draw, &renderer).unwrap(), Some(1));
        assert_eq!(
            picked_slot(&draw, &json!({"model":"@assets/other.glb#as_other001"})).unwrap(),
            None
        );
        assert_eq!(
            picked_slot(
                &draw,
                &json!({"model":"@assets/model.glb#as_model001","node":"Branch","primitive":0})
            )
            .unwrap(),
            None
        );
    }
}
