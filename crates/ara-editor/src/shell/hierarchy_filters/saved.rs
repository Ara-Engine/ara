//! Shared dialogs name saved queries; the settings service owns their persistence.
use super::*;
use ara_ui::dialogs::{DialogFuture, DialogOptions};

#[derive(Default)]
pub(crate) struct View {
    pub(super) reveal: Option<(String, String)>,
    pending: Option<Pending>,
}
enum Pending {
    Name {
        query: String,
        response: DialogFuture<Option<String>>,
    },
    Pick {
        names: Vec<String>,
        remove: bool,
        response: DialogFuture<Option<usize>>,
    },
}
enum Reply {
    Name(String, Option<String>),
    Pick(Vec<String>, bool, Option<usize>),
}

fn filters(editor: &Editor) -> BTreeMap<String, String> {
    editor
        .state
        .settings
        .get("hierarchy.filters")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, query)| query.as_str().map(|query| (name.clone(), query.to_owned())))
        .collect()
}
impl Shell {
    /// Discover saved searches through one searchable kit Select and explicit
    /// Save/Manage actions, rather than a row of exclusive filter buttons.
    pub(in crate::shell) fn hierarchy_saved_filters_markup(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) {
        let saved = filters(editor);
        let query = editor
            .state
            .searches
            .get("hierarchy")
            .map_or("", String::as_str);
        let current = saved
            .iter()
            .find_map(|(name, saved)| (saved == query).then_some(name.as_str()))
            .unwrap_or("");
        let mut options = vec![(
            json!(""),
            if saved.is_empty() {
                "No saved filters".into()
            } else {
                "Saved filters".into()
            },
        )];
        options.extend(saved.keys().map(|name| (json!(name), name.clone())));
        out.push_str("<panel id='hierarchy-saved-filters' class='toolbar' style='min-height:28px;height:auto;gap:4px'><panel id='hierarchy-saved-choice' style='min-width:0px;flex-grow:1'>");
        self.choice_button(
            "hierarchy-saved-filter",
            "Saved hierarchy filter",
            choices::Select {
                destination: choices::Destination::HierarchySavedFilter,
                options,
                current: json!(current),
            },
            out,
        );
        out.push_str("</panel>");
        self.bindings.insert(
            ara_ui::source::stable_id("hierarchy-save-filter"),
            Binding::Command("hierarchy.filter.save", Target::None),
        );
        out.push_str(&format!("<button id='hierarchy-save-filter' class='ghost icon-button' label='Save hierarchy search' description='Save this name, ID or component query for later' action='Dispatch'>{}</button></panel>",icons::icon("hierarchy-save-filter","bookmark")));
    }
    fn prepare_hierarchy_filter_dialog(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.dialog.is_some()
            || self.confirmations.is_open()
            || self.hierarchy_filters.pending.is_some()
            || self.modal_root == Some("build-options")
        {
            return Err(
                "Finish or cancel the current dialog before managing hierarchy filters".into(),
            );
        }
        if editor.state.palette {
            editor.action("editor.palette", json!({"open":false}))?;
        }
        self.menu = None;
        Ok(())
    }
    /// Save the current query by an explicit name, or prompt when invoked without
    /// a text target. Concurrent edits to other saved filters are preserved.
    pub(crate) fn save_hierarchy_filter(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let query = editor
            .state
            .searches
            .get("hierarchy")
            .filter(|query| !query.trim().is_empty())
            .cloned()
            .ok_or("Enter a hierarchy search before saving a filter")?;
        if let Target::Text(name) = target {
            return self.store_hierarchy_filter(editor, name, &query);
        }
        self.prepare_hierarchy_filter_dialog(editor)?;
        let options = DialogOptions::new(
            "Save hierarchy filter",
            format!("Name this query: {query}\nAn existing name updates that saved filter."),
        );
        let response = self.confirmations.prompt(options, String::new());
        self.hierarchy_filters.pending = Some(Pending::Name { query, response });
        self.rebuild(editor)
    }
    fn store_hierarchy_filter(
        &mut self,
        editor: &mut Editor,
        name: &str,
        query: &str,
    ) -> ara_ui::Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err("Use a hierarchy filter name from 1 to 80 characters".into());
        }
        let mut latest = filters(editor);
        latest.insert(name.to_owned(), query.to_owned());
        editor.action(
            "editor.settings.set",
            json!({"key":"hierarchy.filters","value":latest}),
        )?;
        Ok(())
    }
    /// Apply/remove a named filter, or use a shared chooser for palette/menu use.
    pub(crate) fn use_hierarchy_filter(
        &mut self,
        editor: &mut Editor,
        target: &Target,
        remove: bool,
    ) -> ara_ui::Result<()> {
        if let Target::Text(name) = target {
            if name.is_empty() {
                return Ok(());
            }
            let mut latest = filters(editor);
            let query = latest
                .get(name)
                .ok_or("That hierarchy filter no longer exists; choose a saved filter again")?
                .clone();
            if remove {
                latest.remove(name);
                editor.action(
                    "editor.settings.set",
                    json!({"key":"hierarchy.filters","value":latest}),
                )?;
            } else {
                editor.action("editor.search", json!({"panel":"hierarchy","query":query}))?;
                editor.action("editor.layout.show", json!({"panel":"hierarchy"}))?;
                editor.action("editor.focus", json!({"panel":"hierarchy"}))?;
            }
            return Ok(());
        }
        self.prepare_hierarchy_filter_dialog(editor)?;
        let names: Vec<_> = filters(editor).into_keys().collect();
        if names.is_empty() {
            return Err(
                "No saved hierarchy filters. Enter a query and choose Save hierarchy search first"
                    .into(),
            );
        }
        let response = self.confirmations.choose(
            DialogOptions::new(
                if remove {
                    "Delete hierarchy filter"
                } else {
                    "Apply hierarchy filter"
                },
                "Choose a named hierarchy query. Deleting a filter keeps the current search.",
            ),
            names.clone(),
            0,
        );
        self.hierarchy_filters.pending = Some(Pending::Pick {
            names,
            remove,
            response,
        });
        self.rebuild(editor)
    }
    pub(super) fn poll_saved_hierarchy_filters(
        &mut self,
        editor: &mut Editor,
    ) -> ara_ui::Result<bool> {
        let reply = match self.hierarchy_filters.pending.as_mut() {
            Some(Pending::Name { query, response }) => response
                .try_take()
                .map(|name| Reply::Name(query.clone(), name)),
            Some(Pending::Pick {
                names,
                remove,
                response,
            }) => response
                .try_take()
                .map(|index| Reply::Pick(names.clone(), *remove, index)),
            None => None,
        };
        let Some(reply) = reply else {
            return Ok(false);
        };
        self.hierarchy_filters.pending = None;
        match reply {
            Reply::Name(query, Some(name)) => self.store_hierarchy_filter(editor, &name, &query)?,
            Reply::Pick(names, remove, Some(index)) => {
                let name = names
                    .get(index)
                    .ok_or("Choose a valid saved hierarchy filter")?;
                self.use_hierarchy_filter(editor, &Target::Text(name.clone()), remove)?;
            }
            _ => {}
        }
        Ok(true)
    }
}
