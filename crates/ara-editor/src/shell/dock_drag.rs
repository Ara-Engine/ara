//! Dock drop zones and their transient preview; mutations use the layout protocol.
use super::*;
use ara_ui::{paint::Draw, source::stable_id};

/// Where a dragged tab would land; tab indices count the remaining destination tabs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DropTarget {
    /// Insert at a tab-bar caret, or append when dropped on the panel centre.
    Tab { area: String, index: Option<usize> },
    /// Split an area: vertical stacks, `before` puts the panel first.
    Split {
        area: String,
        vertical: bool,
        before: bool,
    },
    /// Release in empty space or outside every window.
    Float,
}

/// The lit region and action label on the destination surface.
#[derive(Clone, Debug)]
pub(crate) struct DropPreview {
    pub target: DropTarget,
    pub rect: Box2,
    pub label: String,
    pub surface: Option<String>,
}

impl Shell {
    /// Tab bars take priority over panel edges, including a lone panel's title.
    pub(crate) fn drop_target(&self, panel: &str) -> Option<(DropTarget, Box2, String)> {
        let [x, y] = self.pointer;
        let area = self.ui.paint_order.iter().rev().find_map(|&i| {
            let n = &self.ui.nodes[i];
            (n.element.id.starts_with("area-")
                && n.element
                    .attrs
                    .get("class")
                    .is_some_and(|c| c.split_whitespace().any(|c| c == "dock"))
                && self.ui.shown(i)
                && n.rect.contains(x, y)
                && n.clip.contains(x, y))
            .then_some(n)
        })?;
        let id = area.element.id.strip_prefix("area-")?;
        let header = area.element.children.first().map(|&i| &self.ui.nodes[i]);
        let title = panels::title(panel);
        if let Some(header) = header.filter(|h| h.rect.contains(x, y)) {
            let mut index = 0;
            let mut caret = header.rect.x + 4.;
            if header.element.id.starts_with("tabs-") {
                for &i in &header.element.children {
                    let tab = &self.ui.nodes[i];
                    let Some(name) = tab.element.id.strip_prefix("tab-") else {
                        continue;
                    };
                    if name == panel || !self.ui.shown(i) {
                        continue;
                    }
                    if x < tab.rect.x + tab.rect.w * 0.5 {
                        caret = tab.rect.x;
                        break;
                    }
                    index += 1;
                    caret = tab.rect.x + tab.rect.w;
                }
            } else if header
                .element
                .id
                .strip_prefix("header-")
                .is_some_and(|p| p != panel)
            {
                // The whole title strip is grabbable, but its text is the tab handle.
                let end = header
                    .element
                    .children
                    .iter()
                    .find_map(|&i| {
                        let n = &self.ui.nodes[i];
                        n.element
                            .id
                            .starts_with("header-title-")
                            .then_some(n.rect.x + n.rect.w + 10.)
                    })
                    .unwrap_or(header.rect.x + 96.);
                if x > (header.rect.x + end) * 0.5 {
                    index = 1;
                    caret = end;
                }
            }
            let rect = Box2 {
                x: caret.clamp(
                    header.rect.x + 2.,
                    (header.rect.x + header.rect.w - 2.).max(header.rect.x + 2.),
                ) - 1.,
                y: header.rect.y + 3.,
                w: 2.,
                h: (header.rect.h - 6.).max(1.),
            };
            return Some((
                DropTarget::Tab {
                    area: id.into(),
                    index: Some(index),
                },
                rect,
                format!("Insert tab · {title}"),
            ));
        }
        let r = area.rect;
        // Edge bands stay easy to acquire without swallowing most of a large panel.
        let ex = (r.w * 0.22).min(80.);
        let ey = (r.h * 0.22).min(80.);
        let edges = [
            (x - r.x, ex, false, true),
            (r.x + r.w - x, ex, false, false),
            (y - r.y, ey, true, true),
            (r.y + r.h - y, ey, true, false),
        ];
        if let Some(&(_, _, vertical, before)) = edges
            .iter()
            .filter(|(d, band, ..)| d < band)
            .min_by(|a, b| (a.0 / a.1.max(1.)).total_cmp(&(b.0 / b.1.max(1.))))
        {
            // Splitting a panel off itself would manufacture an empty half.
            if header.is_some_and(|h| h.element.id.strip_prefix("header-") == Some(panel)) {
                return Some((
                    DropTarget::Tab {
                        area: id.into(),
                        index: None,
                    },
                    r,
                    format!("Keep tab · {title}"),
                ));
            }
            let rect = if vertical {
                Box2 {
                    y: r.y + if before { 0. } else { r.h * 0.5 },
                    h: r.h * 0.5,
                    ..r
                }
            } else {
                Box2 {
                    x: r.x + if before { 0. } else { r.w * 0.5 },
                    w: r.w * 0.5,
                    ..r
                }
            };
            let side = match (vertical, before) {
                (false, true) => "left",
                (false, false) => "right",
                (true, true) => "top",
                (true, false) => "bottom",
            };
            return Some((
                DropTarget::Split {
                    area: id.into(),
                    vertical,
                    before,
                },
                rect,
                format!("Split {side} · {title}"),
            ));
        }
        Some((
            DropTarget::Tab {
                area: id.into(),
                index: None,
            },
            r,
            format!("Add as tab · {title}"),
        ))
    }

    /// Resolve the destination in this surface; the threshold belongs to the source.
    pub fn drag_over(&mut self) -> bool {
        let Some(Drag::Tab {
            panel,
            start,
            surface,
        }) = &self.drag
        else {
            return false;
        };
        if self.drop_preview.is_none()
            && *surface == self.current
            && (self.pointer[0] - start[0]).hypot(self.pointer[1] - start[1])
                < ara_ui::drag_drop::DRAG_THRESHOLD
        {
            return false;
        }
        let (target, rect, label) = self.drop_target(panel).unwrap_or_else(|| {
            (
                DropTarget::Float,
                Box2::default(),
                format!("New window · {}", panels::title(panel)),
            )
        });
        self.drop_preview = Some(DropPreview {
            target,
            rect,
            label,
            surface: self.current.clone(),
        });
        self.invalidate_dock_drag();
        true
    }

    /// Preview detaching beyond every native client rectangle.
    pub(crate) fn drag_outside(&mut self) -> bool {
        let Some(Drag::Tab { panel, .. }) = &self.drag else {
            return false;
        };
        self.drop_preview = Some(DropPreview {
            target: DropTarget::Float,
            rect: Box2::default(),
            label: format!("New window · {}", panels::title(panel)),
            surface: self.current.clone(),
        });
        self.invalidate_dock_drag();
        true
    }

    /// The pressed tab has moved beyond the click threshold.
    pub fn tab_dragging(&self) -> bool {
        self.tab_pending() && self.drop_preview.is_some()
    }

    /// A tab or lone panel title is pressed, including before it starts dragging.
    pub fn tab_pending(&self) -> bool {
        matches!(self.drag, Some(Drag::Tab { .. }))
    }

    /// The stable panel identity held by the pointer.
    pub fn dragged_tab(&self) -> Option<&str> {
        match &self.drag {
            Some(Drag::Tab { panel, .. }) => Some(panel),
            _ => None,
        }
    }

    /// Release capture in the source window too, even when the drop closes it.
    pub(crate) fn release_tab_capture(
        &mut self,
        _clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<()> {
        self.cancel_pointer_capture();
        self.invalidate_dock_drag();
        Ok(())
    }

    /// Commit a resolved drop through the layout protocol, or leave a click alone.
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

    /// Layout changes are typed navigation commands, persisted by the authority.
    pub(crate) fn drop_tab(
        &mut self,
        editor: &mut Editor,
        panel: &str,
        target: &DropTarget,
        rect: Option<[i32; 4]>,
    ) -> ara_ui::Result<()> {
        match target {
            DropTarget::Tab { area, index } => {
                let mut args = json!({"panel": panel, "area": area});
                if let Some(index) = index {
                    args["index"] = json!(index);
                }
                editor.action("editor.layout", args)?;
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
                    json!({"panel": panel, "area": area, "newArea": new_area,
                    "vertical": vertical, "before": before}),
                )?;
            }
            DropTarget::Float => {
                let size = if panel == "scene" {
                    [960, 640]
                } else {
                    [420, 560]
                };
                let rect = rect.unwrap_or_else(|| self.float_rect(size));
                editor.action(
                    "editor.layout.move",
                    json!({"panel": panel, "slot": "float", "rect": rect}),
                )?;
            }
        }
        Ok(())
    }

    fn invalidate_dock_drag(&mut self) {
        if let Some(root) = self.ui.nodes.first_mut() {
            root.dirty.paint = true;
        }
        for surface in &mut self.floats {
            if let Some(root) = surface.ui.nodes.first_mut() {
                root.dirty.paint = true;
            }
        }
    }

    /// Keep the dragged tab's label next to the pointer on the destination surface.
    pub(super) fn sync_tab_ghost(&mut self) -> ara_ui::Result<()> {
        if self.content_pending() {
            return self.sync_content_ghost();
        }
        let Some(i) = self.ui.find("drop-label") else {
            return Ok(());
        };
        let shown = self.tab_dragging()
            && self
                .drop_preview
                .as_ref()
                .is_some_and(|p| p.surface == self.current);
        self.ui.set_visible(i, shown)?;
        if shown {
            if let Some(t) = self.ui.find("tooltip") {
                self.ui.set_visible(t, false)?;
            }
            self.tooltip_text = None;
            if let Some(t) = self.ui.find("drop-label-text")
                && let Some(Drag::Tab { panel, .. }) = &self.drag
            {
                let title = panels::title(panel);
                self.ui.set_text(t, title)?;
            }
            if let Some(icon) = self.ui.find("drop-label-icon")
                && let Some(Drag::Tab { panel, .. }) = &self.drag
            {
                let name = panels::panel_icon(panel);
                let node = &mut self.ui.nodes[icon];
                if node
                    .element
                    .attrs
                    .get("src")
                    .and_then(|s| s.strip_prefix("icon-"))
                    != Some(name)
                {
                    node.element
                        .attrs
                        .insert("src".into(), format!("icon-{name}"));
                    node.dirty.paint = true;
                }
            }
            let node = &mut self.ui.nodes[i];
            let x = (self.pointer[0] + 14.).clamp(0., (self.ui.size[0] - 140.).max(0.));
            let y = (self.pointer[1] + 18.).clamp(0., (self.ui.size[1] - 28.).max(0.));
            let offset = [
                x - (node.rect.x - node.paint_offset[0]),
                y - (node.rect.y - node.paint_offset[1]),
            ];
            if offset != node.paint_offset {
                node.paint_offset = offset;
                node.dirty.paint = true;
            }
        }
        Ok(())
    }

    /// Carets and split rectangles sit above panel contents and below popups.
    pub(super) fn paint_drop_preview(&mut self) {
        self.paint_marquee();
        self.paint_content_drag();
        let Some(preview) = self
            .drop_preview
            .as_ref()
            .filter(|p| p.surface == self.current && p.rect.w > 0.)
        else {
            return;
        };
        let line = self
            .ui
            .sheet
            .color_token("--primary")
            .unwrap_or(ara_ui::style::Color(52, 107, 241, 255));
        let id = stable_id("drop-preview");
        let r = preview.rect;
        if matches!(preview.target, DropTarget::Tab { index: Some(_), .. }) {
            self.ui.display.commands.push(Draw::Rect {
                node: id,
                rect: r,
                color: line,
            });
            for y in [r.y, r.y + r.h - 2.] {
                self.ui.display.commands.push(Draw::Rect {
                    node: id ^ 1,
                    rect: Box2 {
                        x: r.x - 2.,
                        y,
                        w: 6.,
                        h: 2.,
                    },
                    color: line,
                });
            }
        } else {
            let mut fill = line;
            fill.3 = 38;
            let inset = Box2 {
                x: r.x + 3.,
                y: r.y + 3.,
                w: (r.w - 6.).max(0.),
                h: (r.h - 6.).max(0.),
            };
            self.ui.display.commands.push(Draw::RoundedRect {
                node: id,
                rect: inset,
                radius: 6.,
                color: fill,
            });
            self.ui.display.commands.push(Draw::Border {
                node: id ^ 1,
                rect: inset,
                radius: 6.,
                color: line,
                width: 1.5,
            });
        }
    }
}

#[cfg(test)]
#[path = "scene_detach_tests.rs"]
mod scene_detach_tests;
#[cfg(test)]
#[path = "dock_drag_tests.rs"]
mod tests;
