//! Hierarchy: a virtual 22 px tree with type-tinted icons (prefab instances in the
//! prefab tint), a strong selection fill, hover eye/lock toggles, a scene header, a
//! type filter menu (it writes `t:Kind` into the search), a footer count and an
//! empty state.
use super::commands::{Target, menus::Context};
use super::*;
impl Shell {
    pub(super) fn hierarchy_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        out.push_str("<panel id='hierarchy-toolbar' class='toolbar'>");
        self.bindings.insert(
            ara_ui::source::stable_id("entity-create"),
            Binding::ContextMenu(Context::Hierarchy, Target::None),
        );
        out.push_str(&format!("<button id='entity-create' class='ghost' style='padding-left:4px;padding-right:2px' label='Create' description='Create an entity · Shift+A' action='Dispatch'>{}{}</button>",icons::icon("entity-create","plus"),icons::icon_in("entity-create-more","chevron-down","icon-sm")));
        self.search(out, "hierarchy", editor);
        let filtered = editor
            .state
            .searches
            .get("hierarchy")
            .is_some_and(|q| q.starts_with("t:"));
        self.bindings.insert(
            ara_ui::source::stable_id("hierarchy-filter"),
            Binding::ContextMenu(Context::HierarchyFilter, Target::None),
        );
        out.push_str(&format!("<button id='hierarchy-filter' class='{} icon-button' label='Filter by type' description='Show only lights, cameras, meshes, prefabs, audio or effects' action='Dispatch'>{}</button>",if filtered{"selected"}else{"ghost"},if filtered{icons::icon_in("hierarchy-filter","funnel","on-selection")}else{icons::icon("hierarchy-filter","funnel")}));
        out.push_str("</panel>");
        let resolved = self.resolved(editor)?;
        let changed = editor.agent_fields();
        let changed_entities: std::collections::BTreeSet<_> =
            changed.keys().map(|(id, _)| id.as_str()).collect();
        let tints = editor.agent_tints();
        let query = editor
            .state
            .searches
            .get("hierarchy")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        // The row model (hundreds of rows) is rebuilt only when what it shows changed.
        let key = HierarchyKey {
            scene: self.resolved_key(),
            selection: editor.state.selection.clone(),
            collapsed: editor.state.collapsed.clone(),
            expanded: editor.state.expanded.clone(),
            hidden: editor.state.hidden.clone(),
            locked: editor.state.locked.clone(),
            query: query.clone(),
            changed: changed_entities.iter().map(|s| (*s).to_owned()).collect(),
            tints: tints.clone(),
        };
        self.entity_count = resolved.len();
        if self.hierarchy_key.as_ref() != Some(&key) {
            self.hierarchy_key = Some(key);
            self.hierarchy_tints = tints;
            self.hierarchy_ghosts.clear();
            let mut children: BTreeMap<Option<&str>, Vec<&String>> = BTreeMap::new();
            for (id, e) in resolved.iter() {
                children.entry(e.parent.as_deref()).or_default().push(id);
            }
            for kids in children.values_mut() {
                kids.sort_by(|a, b| resolved[*a].order.cmp(&resolved[*b].order).then(a.cmp(b)));
            }
            let mut stack: Vec<_> = children
                .get(&None)
                .into_iter()
                .flatten()
                .rev()
                .map(|id| (*id, 0usize))
                .collect();
            self.hierarchy.items.clear();
            self.hierarchy_style.clear();
            self.selected_rows = editor
                .state
                .selection
                .iter()
                .map(|s| crate::world::render_id(s))
                .collect();
            self.hierarchy_entities.clear();
            while let Some((id, depth)) = stack.pop() {
                let e = &resolved[id];
                let kids = children.get(&Some(id.as_str())).map_or(0, Vec::len);
                let collapsed = editor.state.collapsed.contains(id)
                    || (kids >= LARGE_GROUP && !editor.state.expanded.contains(id));
                let name = e.record.get("name").and_then(Value::as_str).unwrap_or(id);
                let component_filter = query
                    .strip_prefix("type:")
                    .or_else(|| query.strip_prefix("t:"));
                let matches = component_filter.map_or_else(
                    || {
                        query.is_empty()
                            || name.to_lowercase().contains(&query)
                            || id.contains(&query)
                    },
                    |kind| e.record.keys().any(|k| k.to_lowercase().contains(kind)),
                );
                if matches {
                    let key = crate::world::render_id(id);
                    let (icon, tint) = if e.record.contains_key("prefab") {
                        ("package", "tint-prefab")
                    } else if e.record.contains_key("Light") {
                        ("lightbulb", "tint-light")
                    } else if e.record.contains_key("Camera") {
                        ("camera", "tint-camera")
                    } else if e.record.contains_key("VisualEffect") {
                        ("sparkles", "tint-vfx")
                    } else if e.record.contains_key("AudioSource")
                        || e.record.contains_key("AudioListener")
                    {
                        ("volume-2", "tint-audio")
                    } else if e.source.0.as_str() != editor.state.doc {
                        ("package", "tint-prefab")
                    } else if e.record.contains_key("MeshRenderer") {
                        ("box", "")
                    } else if e.record.contains_key("Environment") {
                        ("sun", "tint-light")
                    } else {
                        ("layers", "")
                    };
                    self.hierarchy_style.insert(
                        key,
                        polish::RowStyle {
                            depth,
                            changed: changed_entities.contains(id.as_str()),
                            icon,
                            tint,
                            children: children.contains_key(&Some(id.as_str())),
                            collapsed,
                            locked: editor.state.locked.contains(id),
                            hidden: editor.state.hidden.contains(id),
                        },
                    );
                    self.hierarchy.items.push((key, name.to_owned()));
                    self.hierarchy_entities.insert(key, id.clone());
                }
                if (!collapsed || !query.is_empty())
                    && let Some(kids) = children.get(&Some(id.as_str()))
                {
                    stack.extend(kids.iter().rev().map(|kid| (*kid, depth + 1)));
                }
            }
        }
        // The scene by name ("main", not "main.scene"); its path is the tooltip.
        let path = editor
            .host
            .store()
            .workspace()
            .path_of(&editor.state.doc)
            .unwrap_or("Scene")
            .to_owned();
        let scene = names::stem(names::file_name(&path));
        out.push_str(&format!("<panel id='hierarchy-scene' class='row' style='height:22px;min-height:22px;padding-left:8px;gap:6px;flex-shrink:0' description='{}'>{}<text id='hierarchy-scene-name' class='fg' text='{}' /></panel>",escape(&path),icons::icon_in("hierarchy-scene","clapperboard","icon-sm"),escape(scene)));
        if resolved.is_empty() {
            self.empty(out, "hierarchy-empty", "list-tree", "This scene is empty", "Create an entity, or drag a model in from Assets. Shift+A searches every Create command.", &[("hierarchy-empty-create", "Create entity", "create.empty"), ("hierarchy-empty-cube", "Cube", "create.cube")]);
        } else if self.hierarchy.items.is_empty() {
            self.empty(
                out,
                "hierarchy-none",
                "search",
                "No matching entities",
                "Clear the filter, or search by component with t:Light.",
                &[],
            );
        } else {
            // The row height is in the markup: a density change remounts the row pool.
            let row = if editor.state.comfortable() { 26 } else { 22 };
            out.push_str(&format!("<scroll id='hierarchy-rows' class='rows-{row}' style='flex-grow:1; min-height:0px' />"));
        }
        out.push_str(&format!("<panel id='hierarchy-footer' class='footer'><text id='hierarchy-count' text='{} entities{}' /></panel>",resolved.len(),if editor.state.selection.is_empty(){String::new()}else{format!(" · {} selected",editor.state.selection.len())}));
        Ok(())
    }
    /// Bind the hierarchy rows on screen: select the entity, or open Agents for a
    /// deletion ghost. Only visible rows can be clicked, so rebuilds and scrolls bind
    /// just these (binding every entity cost ~0.3 ms per rebuild on Outpost).
    pub(super) fn bind_hierarchy_rows(&mut self) {
        let Some(view) = &self.hierarchy_view else {
            return;
        };
        for key in view.visible_keys() {
            let Some(entity) = self.hierarchy_entities.get(&key) else {
                continue;
            };
            let binding = if self.hierarchy_ghosts.contains(&key) {
                Binding::Protocol("editor.focus", json!({"panel":"agents"}))
            } else {
                Binding::Select(entity.clone())
            };
            self.bindings.insert(view.identity(key), binding);
        }
    }
    pub(super) fn decorate_hierarchy(&mut self) {
        if self.hierarchy_surface != self.current {
            return;
        }
        let Some(view) = &self.hierarchy_view else {
            return;
        };
        let row_height = view.row_height();
        for index in view.visible_nodes() {
            let key = self.ui.nodes[*index]
                .element
                .id
                .rsplit_once("key-")
                .and_then(|(_, s)| s.parse::<u64>().ok());
            let selected = key.is_some_and(|k| self.selected_rows.contains(&k));
            let class = if selected {
                "virtual-row row-selected"
            } else {
                "virtual-row"
            };
            let node = &mut self.ui.nodes[*index];
            if node.element.attrs.get("class").map(String::as_str) != Some(class) {
                node.element.attrs.insert("class".into(), class.into());
                node.dirty.style = true;
            }
            let depth = key
                .and_then(|k| self.hierarchy_style.get(&k))
                .map_or(0, |s| s.depth);
            let tint = if selected {
                "selection-foreground"
            } else {
                key.and_then(|k| self.hierarchy_tints.get(&k))
                    .copied()
                    .unwrap_or("foreground")
            };
            for position in 0..self.ui.nodes[*index].element.children.len() {
                let child = self.ui.nodes[*index].element.children[position];
                let node = &mut self.ui.nodes[child];
                let style = format!(
                    "height:{row_height}px; width:100%; padding:2px; padding-left:{}px; overflow:hidden; font-size:12px; color:var(--{tint})",
                    44 + depth.min(32) * 16
                );
                if node.element.attrs.get("style") != Some(&style) {
                    node.element.attrs.insert("style".into(), style);
                    node.dirty.style = true;
                }
            }
        }
    }
}

/// A Hierarchy group this large (generated terrain chunks, streamed cells) starts
/// collapsed until the user expands it.
pub const LARGE_GROUP: usize = 32;

/// What the Hierarchy's row model is built from; `hierarchy_panel` rebuilds the rows
/// only when this changes.
#[derive(PartialEq)]
pub(super) struct HierarchyKey {
    scene: Option<(String, u64, usize)>,
    selection: Vec<String>,
    collapsed: std::collections::BTreeSet<String>,
    expanded: std::collections::BTreeSet<String>,
    hidden: std::collections::BTreeSet<String>,
    locked: std::collections::BTreeSet<String>,
    query: String,
    changed: std::collections::BTreeSet<String>,
    tints: BTreeMap<u64, &'static str>,
}
