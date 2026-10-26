use super::*;
use ara_ui::source::stable_id;
use glam::Vec3;
pub(crate) enum Drag {
    Entity {
        entity: String,
        start: [f32; 2],
    },
    Asset {
        asset: assets::Asset,
        start: [f32; 2],
    },
    Tab {
        panel: String,
        start: [f32; 2],
        /// The client surface owning `start`, independent of the current drop surface.
        surface: Option<String>,
    },
    /// Resizing the inspector's label column.
    Column {
        start: [f32; 2],
    },
}
impl Shell {
    /// Scroll a finite row pool over the full hierarchy model.
    pub fn scroll_hierarchy(&mut self, delta: f32) -> ara_ui::Result<bool> {
        if let Some(i) = self.shown_node("asset-grid") {
            let rect = self.ui.nodes[i].rect;
            if rect.contains(self.pointer[0], self.pointer[1]) {
                self.asset_offset = (self.asset_offset + delta)
                    .clamp(0., (self.asset_grid_extent - rect.h).max(0.));
                self.asset_grid_dirty = true;
                return Ok(true);
            }
        }
        if let Some(index) = self.shown_node("asset-rows")
            && let Some(view) = &mut self.asset_view
        {
            let r = self.ui.nodes[index].rect;
            if r.contains(self.pointer[0], self.pointer[1]) {
                self.asset_offset = (self.asset_offset + delta)
                    .clamp(0., (self.asset_rows.items.len() as f32 * 26. - r.h).max(0.));
                view.scroll(&mut self.ui, &self.asset_rows, self.asset_offset, r.h)?;
                self.bind_asset_rows();
                return Ok(true);
            }
        }
        let Some(index) = self.shown_node("hierarchy-rows") else {
            return Ok(false);
        };
        let Some(view) = &mut self.hierarchy_view else {
            return Ok(false);
        };
        let r = self.ui.nodes[index].rect;
        if !r.contains(self.pointer[0], self.pointer[1]) {
            return Ok(false);
        }
        self.hierarchy_offset = (self.hierarchy_offset + delta)
            .clamp(0., (self.hierarchy.items.len() as f32 * 26. - r.h).max(0.));
        view.scroll(&mut self.ui, &self.hierarchy, self.hierarchy_offset, r.h)?;
        self.bind_hierarchy_rows();
        self.decorate_hierarchy();
        Ok(true)
    }
    /// Capture an entity, asset or dock tab as a typed drag payload.
    pub fn begin_drag(&mut self) {
        // A single-panel area's header (`header-{panel}`) is a plain panel, which hit
        // testing skips (it finds focusable nodes): find it under the pointer directly,
        // so a lone panel drags by its header like a tab (a click without movement
        // still reaches the header's buttons).
        let [x, y] = self.pointer;
        if let Some(panel) = self
            .ui
            .nodes
            .iter()
            .rev()
            .find(|n| {
                n.visible
                    && n.element
                        .attrs
                        .get("class")
                        .is_some_and(|c| c == "panel-header")
                    && n.rect.contains(x, y)
            })
            .and_then(|n| n.element.id.strip_prefix("header-"))
        {
            self.drag = Some(Drag::Tab {
                panel: panel.into(),
                start: self.pointer,
                surface: self.current.clone(),
            });
            return;
        }
        let Some(index) = self.ui.hit_test(x, y) else {
            return;
        };
        let id = &self.ui.nodes[index].element.id;
        if let Some(panel) = id.strip_prefix("tab-") {
            self.drag = Some(Drag::Tab {
                panel: panel.into(),
                start: self.pointer,
                surface: self.current.clone(),
            });
            return;
        }
        self.drag = match self.bindings.get(&stable_id(id)) {
            Some(Binding::Select(entity)) => Some(Drag::Entity {
                entity: entity.clone(),
                start: self.pointer,
            }),
            Some(Binding::Asset(asset)) => Some(Drag::Asset {
                asset: asset.clone(),
                start: self.pointer,
            }),
            _ => None,
        };
    }
    /// Resolve a drop into one reparent, instantiate or layout command.
    pub fn drop_at(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(drag) = self.drag.take() else {
            return Ok(false);
        };
        let start = match &drag {
            Drag::Entity { start, .. }
            | Drag::Asset { start, .. }
            | Drag::Tab { start, .. }
            | Drag::Column { start, .. } => start,
        };
        if matches!(drag, Drag::Column { .. }) {
            return Ok(true);
        }
        if (self.pointer[0] - start[0]).hypot(self.pointer[1] - start[1]) < 6. {
            return Ok(false);
        }
        let entity = self
            .ui
            .hit_test(self.pointer[0], self.pointer[1])
            .and_then(|i| self.bindings.get(&stable_id(&self.ui.nodes[i].element.id)))
            .and_then(|b| {
                if let Binding::Select(id) = b {
                    Some(id.clone())
                } else {
                    None
                }
            });
        match drag {
            Drag::Entity { entity: source, .. } => {
                if let Some(target) = entity {
                    let args = if self.modifiers[0] {
                        let resolved =
                            crate::world::resolve(editor.host.store(), &editor.state.doc)
                                .map_err(|d| format!("{d:?}"))?;
                        let parent = resolved.get(&target).and_then(|e| e.parent.as_ref());
                        let index = resolved
                            .iter()
                            .filter(|(_, e)| e.parent.as_ref() == parent)
                            .position(|(id, _)| id == &target)
                            .unwrap_or(0);
                        json!({"entity":source,"parent":parent,"index":index})
                    } else {
                        json!({"entity":source,"parent":target})
                    };
                    editor.apply(
                        vec![json!({"cmd":"entity.reparent","args":args})],
                        "Reparent hierarchy",
                        None,
                    )?;
                }
            }
            Drag::Asset { asset, .. } => {
                let reference = asset.current_reference(editor.host.store().workspace());
                let Some(reference) = reference else {
                    let why = format!(
                        "{} has no asset ID yet: it is still importing. Drop it again when it is ready",
                        asset.name()
                    );
                    self.status_note = Some(why.clone());
                    return Err(why.into());
                };
                if let Some(i) = self.shown_node("animation-clip-field")
                    && self.ui.nodes[i]
                        .rect
                        .contains(self.pointer[0], self.pointer[1])
                    && let Some(Binding::AnimationField(doc, item, key)) =
                        self.bindings.get(&stable_id("animation-clip-field"))
                {
                    let (doc, item, key) = (doc.clone(), item.clone(), key.clone());
                    self.set_animation_field(editor, &doc, &item, &key, json!(reference))?;
                    self.rebuild(editor)?;
                    return Ok(true);
                }
                // Any asset field under the pointer: a field, or an element of one.
                let field = self.bindings.iter().find_map(|(node, binding)| {
                    let Binding::AssetPicker(target, _) = binding else {
                        return None;
                    };
                    let i = self.ui.by_id(*node)?;
                    (self.ui.shown(i)
                        && self.ui.nodes[i]
                            .rect
                            .contains(self.pointer[0], self.pointer[1]))
                    .then(|| target.clone())
                });
                if let Some(field) = field {
                    if let Err(why) = self.asset_for_field(editor, &asset, &field) {
                        self.status_note = Some(why.clone());
                        return Err(why.into());
                    }
                    editor.apply(
                        self.field_commands(&editor.state.selection, &field, json!(reference))?,
                        "Drop asset reference",
                        None,
                    )?;
                    self.rebuild(editor)?;
                    return Ok(true);
                }
                let over_scene = self
                    .viewport()
                    .is_some_and(|r| r.contains(self.pointer[0], self.pointer[1]));
                if (entity.is_some() || over_scene)
                    && matches!(asset.kind.as_str(), "prefab" | "glb" | "gltf" | "fbx")
                {
                    let position = if over_scene {
                        overlay::ground_point(
                            &editor.state.camera,
                            self.viewport().ok_or("viewport missing")?,
                            self.pointer,
                        )
                        .unwrap_or(Vec3::ZERO)
                    } else {
                        Vec3::ZERO
                    };
                    let command = if asset.kind == "prefab" {
                        json!({"cmd":"prefab.instantiate","args":{"prefab":reference,"parent":entity,"position":position.to_array()}})
                    } else {
                        json!({"cmd":"editor.mesh.instantiate","args":{"mesh":reference,"parent":entity,"position":position.to_array()}})
                    };
                    editor.apply(vec![command], "Place asset", None)?;
                }
            }
            Drag::Tab { panel, .. } => {
                if let Some(preview) = self.drop_preview.take() {
                    self.drop_tab(editor, &panel, &preview.target, None)?;
                }
            }
            Drag::Column { .. } => {}
        }
        self.rebuild(editor)?;
        Ok(true)
    }
}
