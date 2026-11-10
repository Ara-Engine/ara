//! Schema component sections with remembered collapse/order and one aligned grid.
//! Controls live in `inspector_fields`; content changes use typed field commands.
use super::commands::{Target, menus::Context};
use super::*;
use crate::inspector::Widget;

/// The icon a component shows in Inspector headings and Add Component, by its kind.
pub(super) fn component_icon(name: &str) -> &'static str {
    let has = |part: &str| name.contains(part);
    match name {
        "Transform" => "move-3d",
        "Light" => "lightbulb",
        "VisualEffect" => "sparkles",
        "Environment" | "HeightFog" => "sun",
        "LocalFogVolume" => "cloud-fog",
        _ if has("PostProcess") => "sliders-horizontal",
        _ if has("Camera") => "camera",
        _ if has("Audio") || has("Acoustic") => "volume-2",
        _ if has("Character") || has("Input") || has("Controller") => "gamepad-2",
        _ if has("Anim") || has("Ragdoll") => "person-standing",
        _ if has("Nav") || has("Agent") => "map",
        _ if has("Terrain") || has("Foliage") || has("Landscape") => "layers",
        _ if has("Net") || has("Replicat") => "wifi",
        _ if has("Script") || has("Behaviour") || has("Brain") => "file-code",
        _ if has("Ui") || has("Canvas") || has("Sprite") => "layout-grid",
        _ => "box",
    }
}
pub(crate) use crate::names::sentence;
impl Shell {
    /// Label column width in logical pixels for an inspector of `width`.
    pub(super) fn label_px(&self, width: f32) -> f32 {
        (width * self.label_width).clamp(96., (width - 120.).max(96.))
    }
    /// The label column for an inspector `width`, and whether vector rows stack.
    /// Three axis cells, a unit and a reset need about 200 px beside a label, as in
    /// Unity's wide mode.
    pub(super) fn row_metrics(&mut self, width: f32) -> f32 {
        let label = self.label_px(width);
        self.stacked_rows = width - label < 200.;
        label
    }
    pub(super) fn inspector_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        if self.import_defaults_open() {
            return self.import_defaults_panel(editor, out);
        }
        if editor.state.focus == "material" {
            return self.material_inspector(editor, out);
        }
        if editor.state.focus == "animation" {
            return self.animation_inspector(editor, out);
        }
        if editor.state.focus == "vfx" {
            return self.vfx_inspector(editor, out);
        }
        if editor.state.focus == "lighting" {
            return self.lighting_inspector(editor, out);
        }
        if editor.state.asset_path.is_some() {
            return self.import_panel(editor, out);
        }
        self.nested_swatches.clear();
        if !editor.document_available() {
            self.empty(
                out,
                "inspector-unavailable",
                "file-question",
                "Scene unavailable",
                "Restore this scene or use Redo to inspect its entities again. The current selection is preserved.",
                &[],
            );
            return Ok(());
        }
        let resolved = self.resolved(editor)?;
        if editor.state.selection.is_empty() {
            self.empty(
                out,
                "inspector-empty",
                "sliders-horizontal",
                "Nothing selected",
                "Select an entity in the Scene or Hierarchy to edit its components.",
                &[("inspector-empty-select", "Select all", "edit.select_all")],
            );
            return Ok(());
        }
        if !editor
            .state
            .selection
            .iter()
            .any(|id| resolved.contains_key(id))
        {
            self.empty(
                out,
                "inspector-selection-unavailable",
                "mouse-pointer",
                "Selection unavailable",
                "The selected entities were removed. Undo restores them, or select another entity to continue.",
                &[],
            );
            return Ok(());
        }
        let width = self
            .shown_node("inspector-scroll")
            .or_else(|| self.shown_node("body-inspector"))
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(320.);
        let label = self.row_metrics(width);
        let first = editor
            .state
            .selection
            .first()
            .and_then(|id| resolved.get(id));
        let name = first
            .and_then(|e| e.record.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Selection");
        let prefab = first.is_some_and(|e| e.source.0.as_str() != editor.state.doc);
        self.bindings.insert(
            ara_ui::source::stable_id("entity-name"),
            Binding::RenameValue,
        );
        out.push_str(&format!("<panel id='inspector-header' class='inspector-header'><panel id='entity-heading' class='row'>{}<field id='entity-name' style='flex-grow:1;flex-basis:0px' text='{}' label='Entity name' description='Rename; Enter applies' action='Dispatch' />",icons::icon("entity-heading",if prefab{"package"}else{"box"}),escape(name)));
        if editor.state.selection.len() > 1 {
            out.push_str(&format!(
                "<text id='selection-count' class='caption' text='{} selected' />",
                editor.state.selection.len()
            ));
        }
        out.push_str("</panel>");
        // Prefab instances (Unity's Overrides bar): which prefab, how many overrides,
        // and Open / Apply all / Revert all.
        if prefab
            && let Some(entity) = editor.state.selection.first()
            && let Some(found) =
                super::prefab_ops::instance(editor.host.store(), &editor.state.doc, entity)
        {
            let file = found
                .prefab_path
                .rsplit('/')
                .next()
                .unwrap_or(&found.prefab_path)
                .to_owned();
            let detail = match (found.inner.is_some(), found.overrides.len()) {
                (true, _) if found.overrides.is_empty() => "inside an instance".to_owned(),
                (_, 0) => "no overrides".to_owned(),
                (_, 1) => "1 override".to_owned(),
                (_, n) => format!("{n} overrides"),
            };
            // The name and count clip in narrow docks; the actions get their own row.
            out.push_str(&format!("<panel id='prefab-bar' class='prefab-bar'><panel id='prefab-head' class='row' style='gap:6px;min-width:0px;overflow:hidden'>{}<text id='prefab-file' class='fg' style='white-space:nowrap;overflow:hidden;min-width:0px;flex-shrink:1' text='{}' /><text id='prefab-overrides' class='caption' style='white-space:nowrap;flex-shrink:0' text='{}' /></panel><panel id='prefab-actions' class='row' style='gap:4px;justify-content:end'>",icons::icon_in("prefab-bar","package","accent"),escape(&file),escape(&detail)));
            for (id, command, label, show) in [
                ("prefab-open", "prefab.open", "Open", true),
                (
                    "prefab-apply",
                    "prefab.apply_all",
                    "Apply all",
                    !found.overrides.is_empty(),
                ),
                (
                    "prefab-revert",
                    "prefab.revert_all",
                    "Revert all",
                    !found.overrides.is_empty(),
                ),
            ] {
                if show {
                    self.bind_command(id, command, super::commands::Target::None);
                    out.push_str(&format!("<button id='{id}' class='notice-action' label='{label}' description='{label}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>"));
                }
            }
            out.push_str("</panel></panel>");
        }
        out.push_str("<panel id='inspector-filter-row' class='row'>");
        self.search(out, "inspector", editor);
        out.push_str("</panel>");
        let changes = editor.agent_fields();
        let doc = editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("document not found")?;
        let mut fields = crate::inspector::fields(
            editor.host.store().schema(),
            doc,
            &resolved,
            &editor.state.selection,
            &changes,
        );
        let mut components: Vec<String> = first
            .map(|entity| {
                entity
                    .record
                    .keys()
                    .filter(|key| ara_doc::is_component_key(key))
                    .filter(|key| {
                        editor.state.selection.iter().all(|id| {
                            resolved
                                .get(id)
                                .is_some_and(|e| e.record.contains_key(*key))
                        })
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.inspector_sections.order(&mut components);
        out.push_str("</panel><scroll id='inspector-scroll' style='min-height:0;flex-grow:1;flex-basis:0px'><panel id='inspector-components' class='inspector-components'>");
        let query = editor
            .state
            .searches
            .get("inspector")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let mut shown = 0;
        for head in components {
            let matching = fields
                .iter()
                .filter(|f| f.path.split('.').next() == Some(head.as_str()))
                .filter(|f| {
                    query.is_empty()
                        || format!(
                            "{} {} {} {}",
                            sentence(&head),
                            sentence(&f.label),
                            f.path,
                            f.doc
                        )
                        .to_lowercase()
                        .contains(&query)
                })
                .collect::<Vec<_>>();
            if !query.is_empty() && matching.is_empty() {
                continue;
            }
            shown += 1;
            if self.field_request != editor.state.focus_field
                && editor
                    .state
                    .focus_field
                    .as_deref()
                    .is_some_and(|path| path.split('.').next() == Some(head.as_str()))
            {
                self.inspector_sections.expand(&head);
            }
            let collapsed = query.is_empty() && self.inspector_sections.collapsed(&head);
            let collapse_id = format!("collapse-component-{head}");
            self.bindings.insert(
                ara_ui::source::stable_id(&collapse_id),
                Binding::ComponentCollapse(head.clone()),
            );
            self.bindings.insert(
                ara_ui::source::stable_id(&format!("component-menu-{head}")),
                Binding::ContextMenu(Context::Component, Target::Component(head.clone())),
            );
            let icon = component_icon(&head);
            let doc = editor
                .host
                .store()
                .schema()
                .get(&head)
                .map_or("", |t| t.doc.as_str());
            let enabled = fields.iter().find(|f| {
                f.path == format!("{head}.enabled")
                    && !f.optional
                    && matches!(f.widget, Widget::Bool)
            });
            let off = enabled.is_some_and(|e| !e.mixed && e.value.as_bool() == Some(false));
            out.push_str(&format!("<panel id='section-{head}' class='component-section{}{}'><panel id='component-{head}' class='component-header'>", if off {" component-off"} else {""}, if collapsed {" component-collapsed"} else {""}));
            // The enable checkbox leads the header, ahead of the title it switches.
            if let Some(enabled) = enabled {
                let id = format!("component-enabled-{head}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Field(enabled.path.clone(), None),
                );
                out.push_str(&format!("<toggle id='{id}' class='component-enabled' checked='{}' label='Enable {}' description='{}' {} action='Dispatch' />", enabled.value.as_bool().unwrap_or(false), escape(&sentence(&head)), escape(&enabled.doc), if enabled.read_only {"disabled='true'"} else {""}));
            }
            out.push_str(&format!("<button id='{collapse_id}' class='component-heading' label='{} {}' description='{}' action='Dispatch'>{}{}<text id='heading-{head}' class='component-title{}' text='{}' /></button>", if collapsed {"Expand"} else {"Collapse"}, escape(&sentence(&head)), escape(doc), icons::icon_in(&collapse_id, if collapsed {"chevron-right"} else {"chevron-down"}, "icon-sm"), icons::icon(&format!("section-icon-{head}"), icon), if off {" component-title-off"} else {""}, escape(&sentence(&head))));
            if enabled.is_some_and(|e| e.mixed) {
                out.push_str(&format!(
                    "<text id='enabled-mixed-{head}' class='caption' text='Mixed' />"
                ));
            }
            out.push_str(&format!("<button id='component-menu-{head}' class='ghost icon-button' label='{head} options' description='Reset, remove, copy, paste or reorder this component' action='Dispatch'>{}</button></panel>", icons::icon(&format!("component-menu-{head}"), "ellipsis")));
            if !collapsed {
                for field in matching {
                    if field.path != format!("{head}.enabled")
                        || field.optional
                        || !matches!(field.widget, Widget::Bool)
                    {
                        self.property_row(editor, field, label, out);
                    }
                }
            }
            out.push_str("</panel>");
        }
        if shown == 0 {
            out.push_str("<panel id='inspector-no-results' class='inspector-no-results'><text id='inspector-no-results-title' text='No matching properties' /><text id='inspector-no-results-help' class='caption' text='Search by component, field name or description.' /></panel>");
        }
        for field in fields.drain(..) {
            self.fields.insert(field.path.clone(), field);
        }
        // Add component follows the last component, as in Unity, not the dock bottom.
        self.bind_command("add-component", "component.add", Target::None);
        out.push_str(&format!("<panel id='add-component-row' class='add-component-row'><button id='add-component' label='Add component' description='Search components to add' action='Dispatch'>{}<text id='add-component-label' text='Add component' /></button></panel>", icons::icon("add-component", "plus")));
        out.push_str("</panel></scroll>");
        Ok(())
    }
}
impl Shell {
    /// Actual laid-out grid edge, including the optional override gutter.
    pub(super) fn label_edge(&self) -> Option<f32> {
        let view = self
            .shown_node("inspector-scroll")
            .map(|index| self.ui.nodes[index].rect)?;
        self.ui.nodes.iter().enumerate().find_map(|(index, node)| {
            (node.element.id.starts_with("field-")
                && self.ui.shown(index)
                && node.rect.x >= view.x
                && node.rect.x < view.x + view.w
                && node.rect.w > 0.)
                .then_some(node.rect.x + node.rect.w)
        })
    }
    /// Start resizing the label column when the pointer is on its edge.
    pub(crate) fn begin_column_drag(&mut self) -> bool {
        let Some(i) = self.shown_node("inspector-scroll") else {
            return false;
        };
        let r = self.ui.nodes[i].rect;
        let edge = self.label_edge().unwrap_or(r.x + self.label_px(r.w));
        if r.contains(self.pointer[0], self.pointer[1]) && (self.pointer[0] - edge).abs() <= 4. {
            self.drag = Some(super::Drag::Column);
            return true;
        }
        false
    }
    /// Follow the pointer while resizing the label column.
    pub(crate) fn drag_column(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(super::Drag::Column) = &self.drag else {
            return Ok(false);
        };
        let Some(i) = self.shown_node("inspector-scroll") else {
            return Ok(false);
        };
        let r = self.ui.nodes[i].rect;
        let ratio = ((self.pointer[0] - r.x) / r.w.max(1.)).clamp(0.2, 0.7);
        if (ratio - self.label_width).abs() < 0.004 {
            return Ok(false);
        }
        self.label_width = ratio;
        self.rebuild(editor)?;
        Ok(true)
    }
}
