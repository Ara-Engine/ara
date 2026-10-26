//! Add Component as a searchable dropdown below the Inspector's button: a filter field,
//! every component the schema registers (minus those the selection already has),
//! and "New Script…" when the scripts command is registered.
use super::*;

/// The command that creates a C# behaviour and adds it to the selection (registered by
/// the Assets/scripts tooling); its row only shows when it exists.
const NEW_SCRIPT: &str = "component.new_script";

/// An open Add Component dropdown.
#[derive(Clone, Debug, Default)]
pub(crate) struct AddComponent {
    /// Typed filter.
    pub query: String,
    /// Keyboard highlight among the shown rows.
    pub active: usize,
}

impl Shell {
    /// Open the dropdown below the Add Component button, or close it when open. Without
    /// a shown button (no Inspector) the palette's component search opens instead.
    pub(crate) fn toggle_add_component(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.add_component.take().is_some() {
            return self.rebuild(editor);
        }
        if self.shown_node("add-component").is_none() {
            self.palette_command = Some("@components".into());
            editor.action("editor.palette", json!({"open": true}))?;
            return self.rebuild(editor);
        }
        // From a shortcut or the palette the button may be scrolled away: bring it in.
        if let Some(button) = self.shown_node("add-component") {
            let node = &self.ui.nodes[button];
            let visible = node.clip.intersection(node.rect);
            if (visible.h < node.rect.h - 0.5)
                && let Some(scroll) = self.ui.find("inspector-scroll")
            {
                self.ui.scroll_to(scroll, [0., 1.0e9])?;
            }
        }
        self.add_component = Some(AddComponent::default());
        self.rebuild(editor)
    }
    /// Components to offer: registered by the schema, not on every selected entity,
    /// matching the filter; with their first doc line.
    fn addable_components(
        &mut self,
        editor: &Editor,
        query: &str,
    ) -> ara_ui::Result<Vec<(String, String)>> {
        let resolved = self.resolved(editor)?;
        let on_all = |name: &str| {
            !editor.state.selection.is_empty()
                && editor
                    .state
                    .selection
                    .iter()
                    .all(|e| resolved.get(e).is_some_and(|r| r.record.contains_key(name)))
        };
        let query = query.to_lowercase();
        let mut out: Vec<(String, String)> = editor
            .host
            .store()
            .schema()
            .components()
            .filter(|t| !on_all(&t.name))
            .filter(|t| {
                query.is_empty() || palette::fuzzy(&t.name.to_lowercase(), &query).is_some()
            })
            .map(|t| {
                (
                    t.name.clone(),
                    t.doc.lines().next().unwrap_or("").to_owned(),
                )
            })
            .collect();
        out.sort();
        Ok(out)
    }
    /// The dropdown's popover markup.
    pub(super) fn add_component_markup(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let Some(state) = self.add_component.clone() else {
            return Ok(());
        };
        // Nothing selected: the dropdown closes. A surface without the Inspector's
        // button (the Inspector floats elsewhere) just leaves it out.
        if editor.state.selection.is_empty() {
            self.add_component = None;
            return Ok(());
        }
        if self.shown_node("add-component").is_none() {
            return Ok(());
        }
        let rows = self.addable_components(editor, &state.query)?;
        let width = self
            .shown_node("add-component")
            .map_or(260., |i| self.ui.nodes[i].rect.w.max(260.));
        self.bindings.insert(
            ara_ui::source::stable_id("add-component-filter"),
            Binding::AddComponentFilter,
        );
        out.push_str(&format!("<popover id='add-component-pop' class='menu add-component' anchor='add-component' placement='bottom-start' offset='4' style='width:{width}px;max-height:420px'><panel id='add-component-filter-row' class='menu-filter'><field id='add-component-filter' class='search-input' style='height:26px' text='{}' label='Search components' action='Dispatch' /><image id='add-component-filter-icon' class='search-icon' src='icon-search' tint='true' />{}</panel>",escape(&state.query),if state.query.is_empty(){"<text id='add-component-placeholder' class='placeholder' text='Search components' />"}else{""}));
        if rows.is_empty() {
            out.push_str("<text id='add-component-empty' class='caption' style='padding:6px' text='No matching components' />");
        }
        for (index, (name, doc)) in rows.iter().enumerate() {
            let id = format!("add-component-{index}");
            let calls = editor
                .state
                .selection
                .iter()
                .map(|entity| json!({"cmd": "component.add", "args": {"entity": entity, "component": name}}))
                .collect();
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Content(calls, format!("Add {name}")),
            );
            let active = if index == state.active {
                " menu-active"
            } else {
                ""
            };
            out.push_str(&format!("<button id='{id}' class='menu-item{active}' label='Add {}' description='{}' action='Dispatch'>{}<text id='{id}-label' class='menu-label' text='{}' /></button>",escape(name),escape(doc),icons::icon(&format!("{id}-icon"),"box"),escape(name)));
        }
        if commands::get(NEW_SCRIPT).is_some() {
            self.bind_command("add-component-script", NEW_SCRIPT, commands::Target::None);
            out.push_str("<panel id='add-component-sep' class='menu-sep'><panel id='add-component-line' class='menu-line' /></panel>");
            out.push_str(&format!("<button id='add-component-script' class='menu-item' label='New Script' description='Create a C# behaviour and add it to the selection' action='Dispatch'>{}<text id='add-component-script-label' class='menu-label' text='New Script…' /></button>",icons::icon("add-component-script-icon","file-plus")));
        }
        out.push_str("</popover>");
        Ok(())
    }
    /// Keys while the dropdown is open: arrows move the highlight, Enter adds it,
    /// Escape closes. Returns whether the key was used.
    pub(crate) fn add_component_key(
        &mut self,
        editor: &mut Editor,
        scancode: i32,
    ) -> ara_ui::Result<bool> {
        let Some(state) = self.add_component.clone() else {
            return Ok(false);
        };
        let rows = self.addable_components(editor, &state.query)?;
        match scancode {
            81 | 82 if !rows.is_empty() => {
                let n = rows.len();
                let active = if scancode == 81 {
                    (state.active + 1) % n
                } else {
                    (state.active + n - 1) % n
                };
                if let Some(s) = &mut self.add_component {
                    s.active = active;
                }
            }
            40 | 88 => {
                if let Some((name, _)) = rows.get(state.active.min(rows.len().saturating_sub(1))) {
                    let calls = editor
                        .state
                        .selection
                        .iter()
                        .map(|entity| json!({"cmd": "component.add", "args": {"entity": entity, "component": name}}))
                        .collect();
                    self.add_component = None;
                    editor.apply(calls, &format!("Add {name}"), None)?;
                }
            }
            41 => self.add_component = None,
            _ => return Ok(false),
        }
        self.rebuild(editor)?;
        Ok(true)
    }
}
