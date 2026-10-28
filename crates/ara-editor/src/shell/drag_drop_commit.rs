//! Commit accepted gestures through typed authoring commands.
use super::*;
impl Shell {
    pub(in crate::shell) fn finish_content_drag(
        &mut self,
        editor: &mut Editor,
    ) -> ara_ui::Result<bool> {
        self.content_drag_over(editor, 0.)?;
        let Some(Drag::Content(drag)) = self.drag.take() else {
            return Ok(false);
        };
        if !drag.session.active() {
            return Ok(false);
        }
        if let Some(Feedback::Reject { reason, .. }) = drag.session.feedback() {
            self.status_note = Some(reason.clone());
        }
        let Some(drop) = drag.session.finish() else {
            self.invalidate_content_drag();
            self.rebuild(editor)?;
            return Ok(true);
        };
        if let Payload::Asset(asset) = &drop.payload
            && !editor.host.root().join(&asset.path).exists()
        {
            self.status_note =
                Some("The dragged asset was removed; choose an existing asset".into());
            self.invalidate_content_drag();
            self.rebuild(editor)?;
            return Ok(true);
        }
        match (drop.payload, drop.target) {
            (Payload::Asset(asset), Target::AssetFolder(folder)) => {
                self.move_asset_files_to(editor, &asset, &folder)?
            }
            (Payload::Entity(source), Target::Hierarchy { entity, placement }) => {
                let resolved = self.resolved(editor)?;
                let calls = hierarchy::commands(
                    &resolved,
                    &editor.state.selection,
                    &source,
                    entity.as_deref(),
                    placement,
                    &editor.state.locked,
                )?;
                editor.apply(calls, "Move hierarchy selection", None)?;
            }
            (Payload::Entity(source), Target::EntityField(field)) => {
                let resolved = self.resolved(editor)?;
                if !resolved.contains_key(&source) {
                    return Err("The dragged entity was deleted".into());
                }
                editor.apply(
                    self.field_commands(editor, &editor.state.selection, &field, json!(source))?,
                    "Drop entity reference",
                    None,
                )?;
            }
            (Payload::Asset(asset), Target::AssetField(field)) => {
                let reference = self.asset_for_field(editor, &asset, &field)?;
                editor.apply(
                    self.field_commands(editor, &editor.state.selection, &field, json!(reference))?,
                    "Drop asset reference",
                    None,
                )?;
            }
            (Payload::Asset(asset), Target::Animation { doc, item, key }) => {
                let reference = asset
                    .current_reference(editor.host.store().workspace())
                    .ok_or("The asset is no longer available")?;
                self.set_animation_field(editor, &doc, &item, &key, json!(reference))?;
            }
            (Payload::Asset(asset), Target::Hierarchy { entity, placement }) => {
                let resolved = self.resolved(editor)?;
                let (parent, index) =
                    hierarchy::destination(&resolved, entity.as_deref(), placement)?;
                let reference = asset
                    .current_reference(editor.host.store().workspace())
                    .ok_or("The asset is no longer available")?;
                editor.apply(vec![if asset.kind=="prefab" { json!({"cmd":"prefab.instantiate","args":{"prefab":reference,"parent":parent,"index":index,"position":[0.,0.,0.]}}) } else { json!({"cmd":"editor.mesh.instantiate","args":{"mesh":reference,"parent":parent,"index":index,"position":[0.,0.,0.]}}) }],"Place asset",None)?;
            }
            (Payload::Asset(asset), Target::Viewport(position)) => {
                let reference = asset
                    .current_reference(editor.host.store().workspace())
                    .ok_or("The asset is no longer available")?;
                editor.apply(vec![if asset.kind=="prefab" { json!({"cmd":"prefab.instantiate","args":{"prefab":reference,"position":position}}) } else { json!({"cmd":"editor.mesh.instantiate","args":{"mesh":reference,"position":position}}) }],"Place asset",None)?;
            }
            (Payload::Component(source), Target::Component { name, after }) => {
                self.reorder_component(&source, &name, after)?
            }
            (Payload::Asset(asset), Target::Material { entity, primitive }) => {
                self.commit_material_drop(editor, &asset, &entity, primitive)?
            }
            (Payload::Collection(source), Target::Collection { row, after }) => {
                self.collection_reorder(editor, &source, &row, after)?
            }
            _ => return Err("The drop target changed; try the gesture again".into()),
        }
        self.rebuild(editor)?;
        Ok(true)
    }
}
