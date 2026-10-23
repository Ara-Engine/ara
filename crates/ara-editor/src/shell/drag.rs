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
    /// Resizing the inspector's label column.
    Column {
        start: [f32; 2],
    },
}
/// Where a dragged tab would land.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DropTarget {
    /// Add as a tab of an area.
    Tab(String),
    /// Split an area: vertical stacks, `before` puts the panel first.
    Split {
        area: String,
        vertical: bool,
        before: bool,
    },
    /// Release outside every window: a new floating window.
    Float,
}
/// The lit region and action label shown while a tab is dragged.
#[derive(Clone, Debug)]
pub(crate) struct DropPreview {
    pub target: DropTarget,
    pub rect: Box2,
    pub label: String,
    /// Surface showing the preview (`None` is the main window).
    pub surface: Option<String>,
}
impl Shell {
    /// Scroll a finite row pool over the full hierarchy model.
    pub fn scroll_hierarchy(&mut self, delta: f32) -> ara_ui::Result<bool> {
        if let Some(i) = self.ui.find("asset-grid") {
            let rect = self.ui.nodes[i].rect;
            if rect.contains(self.pointer[0], self.pointer[1]) {
                self.asset_offset = (self.asset_offset + delta)
                    .clamp(0., (self.asset_grid_extent - rect.h).max(0.));
                self.asset_grid_dirty = true;
                return Ok(true);
            }
        }
        if let Some(view) = &mut self.asset_view
            && let Some(index) = self.ui.find("asset-rows")
        {
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
        if let Some(panel) = id
            .strip_prefix("tab-")
            .or_else(|| id.strip_prefix("header-"))
        {
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
                    return Err(
                        "Mesh/prefab needs a stable asset ID from its document or import sidecar"
                            .into(),
                    );
                };
                if let Some(i) = self.ui.find("animation-clip-field")
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
                let field = self
                    .fields
                    .values()
                    .find(|field| {
                        matches!(field.widget, crate::inspector::Widget::Asset { .. })
                            && self
                                .ui
                                .find(&format!("asset-field-{}", field.path.replace('.', "-")))
                                .is_some_and(|i| {
                                    self.ui.nodes[i]
                                        .rect
                                        .contains(self.pointer[0], self.pointer[1])
                                })
                    })
                    .map(|f| f.path.clone());
                if let Some(field) = field {
                    editor.apply(
                        crate::inspector::edit_commands(
                            &editor.state.selection,
                            &field,
                            &json!(reference),
                        ),
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
impl Shell {
    /// Resolve the drop target under the pointer of the current surface.
    pub(crate) fn drop_target(&self, panel: &str) -> Option<(DropTarget, Box2, String)> {
        let p = self.pointer;
        let area = self.ui.nodes.iter().rev().find(|n| {
            n.element.id.starts_with("area-") && n.visible && n.rect.contains(p[0], p[1])
        })?;
        let id = area.element.id.trim_start_matches("area-").to_owned();
        let r = area.rect;
        let name = panels::title(panel).to_owned();
        let dx = (p[0] - r.x) / r.w.max(1.);
        let dy = (p[1] - r.y) / r.h.max(1.);
        let edge = 0.28;
        let (target, rect, label) = if dx < edge {
            (
                DropTarget::Split {
                    area: id,
                    vertical: false,
                    before: true,
                },
                Box2 { w: r.w * 0.5, ..r },
                format!("Split left · {name}"),
            )
        } else if dx > 1. - edge {
            (
                DropTarget::Split {
                    area: id,
                    vertical: false,
                    before: false,
                },
                Box2 {
                    x: r.x + r.w * 0.5,
                    w: r.w * 0.5,
                    ..r
                },
                format!("Split right · {name}"),
            )
        } else if dy < edge {
            (
                DropTarget::Split {
                    area: id,
                    vertical: true,
                    before: true,
                },
                Box2 { h: r.h * 0.5, ..r },
                format!("Split up · {name}"),
            )
        } else if dy > 1. - edge {
            (
                DropTarget::Split {
                    area: id,
                    vertical: true,
                    before: false,
                },
                Box2 {
                    y: r.y + r.h * 0.5,
                    h: r.h * 0.5,
                    ..r
                },
                format!("Split down · {name}"),
            )
        } else {
            (DropTarget::Tab(id), r, format!("Add as tab · {name}"))
        };
        Some((target, rect, label))
    }
    /// Update the live preview for a tab drag at the current surface's pointer.
    pub(crate) fn drag_over(&mut self) -> bool {
        let Some(Drag::Tab { panel, start }) = &self.drag else {
            return false;
        };
        if self.current.is_none()
            && (self.pointer[0] - start[0]).hypot(self.pointer[1] - start[1]) < 6.
        {
            return false;
        }
        let panel = panel.clone();
        let next = self
            .drop_target(&panel)
            .map(|(target, rect, label)| DropPreview {
                target,
                rect,
                label,
                surface: self.current.clone(),
            });
        let changed = match (&next, &self.drop_preview) {
            (Some(a), Some(b)) => a.target != b.target || a.surface != b.surface,
            (None, None) => false,
            _ => true,
        };
        self.drop_preview = next;
        if changed {
            for n in &mut self.ui.nodes {
                n.dirty.paint = true;
            }
        }
        changed
    }
    /// Preview a release outside every window: a new floating window.
    pub(crate) fn drag_outside(&mut self) -> bool {
        let Some(Drag::Tab { panel, .. }) = &self.drag else {
            return false;
        };
        let label = format!("New window · {}", panels::title(panel));
        let changed = !matches!(&self.drop_preview, Some(p) if p.target == DropTarget::Float);
        self.drop_preview = Some(DropPreview {
            target: DropTarget::Float,
            rect: Box2::default(),
            label,
            surface: None,
        });
        changed
    }
    /// The panel being dragged, if a tab drag is active.
    pub fn dragged_tab(&self) -> Option<&str> {
        match &self.drag {
            Some(Drag::Tab { panel, .. }) => Some(panel),
            _ => None,
        }
    }
    /// Finish a tab drag with the current preview; `rect` places a new window.
    pub fn finish_tab_drag(
        &mut self,
        editor: &mut Editor,
        rect: Option<[i32; 4]>,
    ) -> ara_ui::Result<bool> {
        let Some(Drag::Tab { panel, .. }) = self.drag.take() else {
            return Ok(false);
        };
        let Some(preview) = self.drop_preview.take() else {
            return Ok(false);
        };
        self.drop_tab(editor, &panel, &preview.target, rect)?;
        self.rebuild(editor)?;
        Ok(true)
    }
    /// Apply a tab drop through the layout protocol; `rect` places new windows.
    pub(crate) fn drop_tab(
        &mut self,
        editor: &mut Editor,
        panel: &str,
        target: &DropTarget,
        rect: Option<[i32; 4]>,
    ) -> ara_ui::Result<()> {
        match target {
            DropTarget::Tab(area) => {
                editor.action("editor.layout", json!({"panel": panel, "area": area}))?;
            }
            DropTarget::Split {
                area,
                vertical,
                before,
            } => {
                let new_area = format!(
                    "{panel}-{}",
                    editor.host.store().revision() + editor.view_revision
                );
                editor.action(
                    "editor.layout",
                    json!({"panel": panel, "area": area, "newArea": new_area, "vertical": vertical, "before": before}),
                )?;
            }
            DropTarget::Float => {
                if panel == "scene" {
                    return Err(
                        "The Scene viewport stays in the main window; float tool panels and graphs"
                            .into(),
                    );
                }
                let rect = rect.unwrap_or_else(|| self.float_rect([420, 560]));
                editor.action(
                    "editor.layout.move",
                    json!({"panel": panel, "slot": "float", "rect": rect}),
                )?;
            }
        }
        Ok(())
    }
    /// Lit drop region over the target area of this surface.
    pub(super) fn paint_drop_preview(&mut self) {
        let Some(preview) = &self.drop_preview else {
            if let Some(i) = self.ui.find("drop-label")
                && self.ui.nodes[i].visible
            {
                let _ = self.ui.set_visible(i, false);
            }
            return;
        };
        if preview.surface != self.current || preview.rect.w <= 0. {
            return;
        }
        let token = |name: &str| {
            self.ui
                .sheet
                .color_token(name)
                .unwrap_or(ara_ui::style::Color(52, 107, 241, 255))
        };
        let mut fill = token("--primary");
        fill.3 = 46;
        let line = token("--primary");
        let r = preview.rect;
        let inset = Box2 {
            x: r.x + 3.,
            y: r.y + 3.,
            w: (r.w - 6.).max(0.),
            h: (r.h - 6.).max(0.),
        };
        let id = ara_ui::source::stable_id("drop-preview");
        self.ui
            .display
            .commands
            .push(ara_ui::paint::Draw::RoundedRect {
                node: id,
                rect: inset,
                radius: 8.,
                color: fill,
            });
        self.ui.display.commands.push(ara_ui::paint::Draw::Border {
            node: id ^ 1,
            rect: inset,
            radius: 8.,
            color: line,
            width: 2.,
        });
        let label = preview.label.clone();
        if let Some(i) = self.ui.find("drop-label") {
            if let Some(t) = self.ui.find("drop-label-text") {
                let _ = self.ui.set_text(t, &label);
            }
            let _ = self.ui.set_visible(i, true);
            let node = &mut self.ui.nodes[i];
            node.paint_offset = [
                inset.x + inset.w * 0.5 - 90. - (node.rect.x - node.paint_offset[0]),
                inset.y + inset.h * 0.5 - 12. - (node.rect.y - node.paint_offset[1]),
            ];
        }
    }
}
