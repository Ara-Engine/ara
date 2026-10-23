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
    },
}
impl Shell {
    /// Scroll a finite row pool over the full hierarchy model.
    pub fn scroll_hierarchy(&mut self, delta: f32) -> ara_ui::Result<bool> {
        if let Some(view) = &mut self.asset_view
            && let Some(index) = self.ui.find("asset-rows") {
                let r = self.ui.nodes[index].rect;
                if r.contains(self.pointer[0], self.pointer[1]) {
                    self.asset_offset = (self.asset_offset + delta)
                        .clamp(0., (self.asset_rows.items.len() as f32 * 26. - r.h).max(0.));
                    view.scroll(&mut self.ui, &self.asset_rows, self.asset_offset, r.h)?;
                    return Ok(true);
                }
            }
        let Some(view) = &mut self.hierarchy_view else {
            return Ok(false);
        };
        let Some(index) = self.ui.find("hierarchy-rows") else {
            return Ok(false);
        };
        let r = self.ui.nodes[index].rect;
        if !r.contains(self.pointer[0], self.pointer[1]) {
            return Ok(false);
        }
        self.hierarchy_offset = (self.hierarchy_offset + delta)
            .clamp(0., (self.hierarchy.items.len() as f32 * 26. - r.h).max(0.));
        view.scroll(&mut self.ui, &self.hierarchy, self.hierarchy_offset, r.h)?;
        self.decorate_hierarchy();
        Ok(true)
    }
    /// Capture an entity, asset or dock tab as a typed drag payload.
    pub fn begin_drag(&mut self) {
        let Some(index) = self.ui.hit_test(self.pointer[0], self.pointer[1]) else {
            return;
        };
        let id = &self.ui.nodes[index].element.id;
        if let Some(panel) = id.strip_prefix("tab-") {
            self.drag = Some(Drag::Tab {
                panel: panel.into(),
                start: self.pointer,
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
            Drag::Entity { start, .. } | Drag::Asset { start, .. } | Drag::Tab { start, .. } => {
                start
            }
        };
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
                let Some(reference) = asset.reference else {
                    return Err(
                        "Mesh/prefab needs a stable asset ID from its document or import sidecar"
                            .into(),
                    );
                };
                let over_scene = self
                    .viewport()
                    .is_some_and(|r| r.contains(self.pointer[0], self.pointer[1]));
                if entity.is_some() || over_scene {
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
                let target = self.ui.nodes.iter().find(|n| {
                    n.element.id.starts_with("area-")
                        && n.rect.contains(self.pointer[0], self.pointer[1])
                });
                if let Some(target) = target {
                    let area = target.element.id.trim_start_matches("area-");
                    let r = target.rect;
                    let dx = (self.pointer[0] - r.x).min(r.x + r.w - self.pointer[0]);
                    let dy = (self.pointer[1] - r.y).min(r.y + r.h - self.pointer[1]);
                    let params = if dx < 30. || dy < 30. {
                        let vertical = dy < dx;
                        let before = if vertical {
                            self.pointer[1] < r.y + r.h * 0.5
                        } else {
                            self.pointer[0] < r.x + r.w * 0.5
                        };
                        json!({"panel":panel,"area":area,"newArea":format!("dock-{}",editor.host.store().revision()),"vertical":vertical,"before":before})
                    } else {
                        json!({"panel":panel,"area":area})
                    };
                    editor.action("editor.layout", params)?;
                }
            }
        }
        self.rebuild(editor)?;
        Ok(true)
    }
}
