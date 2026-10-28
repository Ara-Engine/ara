use super::*;
use proof::require;

impl HierarchyCheck {
    pub(super) fn query(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            1 => {
                self.replace(shell, window, "search-hierarchy", QUERY, scale, events)?;
            }
            2 | 4 => {
                require(
                    proof::row_ids(shell) == [QUERY_ROOT, QUERY_BRANCH, QUERY_LEAF],
                    "Filtered Hierarchy lost its real ancestors or retained a nonmatching sibling",
                )?;
                let rows = proof::rows(shell);
                let padding = rows
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|row| row["paddingLeft"].as_f64())
                    .collect::<Vec<_>>();
                require(
                    padding == [Some(44.), Some(60.), Some(76.)],
                    "Filtered Hierarchy flattened the matched row's tree depth",
                )?;
                require(
                    editor.state.collapsed == self.before.collapsed,
                    "Typing a filter rewrote collapse preferences",
                )?;
                require(
                    editor.state.hidden == self.before.hidden
                        && editor.state.selection == self.before.selection,
                    "Filtering changed visibility or selected content",
                )?;
                proof::unchanged(self, editor)?;
                self.mark(if self.phase==2 {"interaction"}else{"id-query"},editor,shell,json!({"query":editor.state.searches.get("hierarchy"),"ancestorDepths":[0,1,2],"nonmatchingSiblingExcluded":true}));
                self.next();
            }
            3 => {
                self.replace(
                    shell,
                    window,
                    "search-hierarchy",
                    "ID:E_HQUERYLEAF -t:Camera",
                    scale,
                    events,
                )?;
            }
            5 => {
                self.replace(
                    shell,
                    window,
                    "search-hierarchy",
                    "name:NoMatchingNativeEntity",
                    scale,
                    events,
                )?;
            }
            6 => {
                require(
                    proof::row_ids(shell).is_empty()
                        && shell.shown_node("hierarchy-none").is_some(),
                    "The nonmatching query did not expose its real empty state",
                )?;
                require(
                    editor.state.selection == self.before.selection,
                    "Filtering out a selected row cleared the selection",
                )?;
                self.key(window, 14, true, events)?;
            }
            7 => {
                self.replace(
                    shell,
                    window,
                    "palette-search",
                    "hierarchy.reveal",
                    scale,
                    events,
                )?;
            }
            8 => {
                self.click_at(
                    window,
                    input::command(shell, "hierarchy.reveal", true),
                    scale,
                    false,
                    "Palette > Reveal in Hierarchy",
                    events,
                )?;
            }
            _ => {
                if proof::focus(shell).as_deref() != Some(QUERY_LEAF)
                    || input::row(shell, QUERY_LEAF).is_none()
                {
                    return Ok(());
                }
                require(
                    editor
                        .state
                        .searches
                        .get("hierarchy")
                        .is_none_or(|query| query.is_empty()),
                    "Reveal did not clear the filter hiding its target",
                )?;
                for id in [QUERY_ROOT, QUERY_BRANCH, QUERY_LEAF] {
                    require(
                        !editor.state.hidden.contains(id) && !editor.state.collapsed.contains(id),
                        "Reveal did not unhide and expand its live ancestor chain",
                    )?;
                }
                require(
                    editor.state.locked == self.before.locked
                        && editor.state.selection == self.before.selection,
                    "Reveal changed locks or selected content",
                )?;
                proof::unchanged(self, editor)?;
                self.mark("revealed",editor,shell,json!({"entity":QUERY_LEAF,"focusedVisibleStableRow":true,"ancestorsUnhidden":true,"selectionPreserved":true}));
                self.pass();
            }
        }
        Ok(())
    }

    pub(super) fn saved(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            1 => {
                self.replace(shell, window, "search-hierarchy", QUERY, scale, events)?;
            }
            2 | 6 => {
                require(
                    editor.state.searches.get("hierarchy").map(String::as_str) == Some(QUERY),
                    "Save would capture a different hierarchy query",
                )?;
                self.click_at(
                    window,
                    input::named(shell, "hierarchy-save-filter"),
                    scale,
                    false,
                    "Hierarchy > Save search",
                    events,
                )?;
            }
            3 | 7 => {
                let Some(id) = input::dialog_field(shell, "Save hierarchy filter") else {
                    return Ok(());
                };
                self.replace(
                    shell,
                    window,
                    &id,
                    if self.phase == 3 {
                        "Native cancelled hierarchy filter"
                    } else {
                        SAVED
                    },
                    scale,
                    events,
                )?;
            }
            4 => self.key(window, 41, false, events)?,
            5 => {
                if shell.modal_active() {
                    return Ok(());
                }
                let current = editor
                    .state
                    .settings
                    .get("hierarchy.filters")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                require(
                    current == self.original_filters,
                    "Escape persisted a cancelled saved-query prompt",
                )?;
                self.mark(
                    "save-cancelled",
                    editor,
                    shell,
                    json!({"savedFiltersUnchanged":true}),
                );
                self.next();
            }
            8 => self.key(window, 40, false, events)?,
            9 => {
                if shell.modal_active() {
                    return Ok(());
                }
                require(
                    editor
                        .state
                        .settings
                        .get("hierarchy.filters")
                        .and_then(|value| value.get(SAVED))
                        .and_then(Value::as_str)
                        == Some(QUERY),
                    "The named query was not saved through the shared prompt",
                )?;
                self.mark(
                    "interaction",
                    editor,
                    shell,
                    json!({"savedName":SAVED,"query":QUERY,"sharedPrompt":true}),
                );
                self.next();
            }
            10 => {
                self.replace(shell, window, "search-hierarchy", "", scale, events)?;
            }
            11 => {
                let Some(index) = shell.shown_node("hierarchy-saved-filter") else {
                    return Ok(());
                };
                require(
                    shell.ui.nodes[index].element.tag == "dropdown",
                    "Saved filters did not use the shared Select control",
                )?;
                self.click_at(
                    window,
                    input::named(shell, "hierarchy-saved-filter"),
                    scale,
                    false,
                    "Saved filters dropdown",
                    events,
                )?;
            }
            12 => {
                self.replace(
                    shell,
                    window,
                    "choice-hierarchy-saved-filter-search",
                    SAVED,
                    scale,
                    events,
                )?;
            }
            13 => {
                self.click_at(
                    window,
                    input::text_control(
                        shell,
                        "ara.select.choice-hierarchy-saved-filter.choose",
                        SAVED,
                    ),
                    scale,
                    false,
                    "Select saved hierarchy query",
                    events,
                )?;
            }
            14 => {
                require(
                    editor.state.searches.get("hierarchy").map(String::as_str) == Some(QUERY)
                        && !shell.choice_open(),
                    "The native saved-filter Select did not restore the exact query",
                )?;
                require(
                    proof::row_ids(shell) == [QUERY_ROOT, QUERY_BRANCH, QUERY_LEAF],
                    "Applying a saved query changed its ancestor projection",
                )?;
                self.mark(
                    "saved-applied",
                    editor,
                    shell,
                    json!({"queryRestored":true,"sharedSelect":true}),
                );
                self.next();
            }
            15 => self.key(window, 14, true, events)?,
            16 => {
                self.replace(
                    shell,
                    window,
                    "palette-search",
                    "hierarchy.filter.remove",
                    scale,
                    events,
                )?;
            }
            17 => {
                self.click_at(
                    window,
                    input::command(shell, "hierarchy.filter.remove", true),
                    scale,
                    false,
                    "Palette > Delete Saved Hierarchy Filter",
                    events,
                )?;
            }
            18 => {
                self.click_at(
                    window,
                    input::text_control(shell, "ara.dialog.choose", SAVED),
                    scale,
                    false,
                    "Shared chooser > named saved query",
                    events,
                )?;
            }
            _ => {
                if shell.modal_active() {
                    return Ok(());
                }
                let current = editor
                    .state
                    .settings
                    .get("hierarchy.filters")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                require(
                    current == self.original_filters,
                    "Deleting one saved query changed unrelated saved filters",
                )?;
                require(
                    editor.state.searches.get("hierarchy").map(String::as_str) == Some(QUERY),
                    "Deleting a saved filter erased the active search",
                )?;
                proof::unchanged(self, editor)?;
                require(
                    editor.state.selection == self.before.selection,
                    "Saved filter management changed selected content",
                )?;
                self.pass();
            }
        }
        Ok(())
    }
}
