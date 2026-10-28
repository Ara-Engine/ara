use super::*;
use input::{command, row, submenu};
use proof::require;

impl HierarchyCheck {
    pub(super) fn groups(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        _scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            1 => self.key(window, 10, true, events)?, // Ctrl+G
            2 => {
                let after = fixture::records(editor)?;
                let added = after
                    .as_object()
                    .into_iter()
                    .flat_map(|records| records.keys())
                    .filter(|id| self.before.records.get(*id).is_none())
                    .cloned()
                    .collect::<Vec<_>>();
                require(
                    added.len() == 1,
                    "Native Group did not create exactly one container",
                )?;
                let id = added[0].clone();
                let entities = fixture::entities(editor)?;
                require(
                    editor.host.store().revision() == self.before.revision + 1,
                    "Group was not one content transaction",
                )?;
                require(
                    entities
                        .get(&id)
                        .is_some_and(|entity| entity.parent.as_deref() == Some(COMMON)),
                    "Cross-parent Group used the wrong shared ancestor",
                )?;
                require(
                    proof::children(&entities, &id) == [FIRST, LINKED],
                    "Group duplicated a selected descendant or changed source order",
                )?;
                require(
                    proof::children(&entities, COMMON) == [id.as_str(), LEFT, RIGHT, OTHER],
                    "Group was not inserted before the first selected branch",
                )?;
                require(
                    proof::same_world(self, &entities),
                    "Grouping changed a source or descendant world pose",
                )?;
                require(
                    after[LINKED]["prefab"] == self.before.records[LINKED]["prefab"],
                    "Grouping lost the linked prefab reference",
                )?;
                require(
                    after.get(&self.linked_inner).is_none()
                        && entities.contains_key(&self.linked_inner),
                    "Grouping flattened the selected prefab child",
                )?;
                require(
                    editor.state.selection == [id.clone()] && editor.state.expanded.contains(&id),
                    "Group did not select and expand its new container",
                )?;
                self.group = Some(id.clone());
                self.committed = Some(after);
                self.mark("interaction",editor,shell,json!({"group":id,"roots":[FIRST,LINKED],"worldPosesPreserved":true,"linkedPrefabPreserved":true,"oneTransaction":true}));
                self.next();
            }
            3 => self.key(window, 29, true, events)?,
            4 => {
                require(
                    fixture::records(editor)? == self.before.records
                        && editor.host.store().revision() == self.before.revision + 2,
                    "One native Undo did not restore the complete pre-group scene",
                )?;
                self.mark(
                    "undo",
                    editor,
                    shell,
                    json!({"completeOriginalRecordsRestored":true}),
                );
                self.next();
            }
            5 => self.key(window, 28, true, events)?,
            _ => {
                require(
                    self.committed.as_ref() == Some(&fixture::records(editor)?)
                        && editor.host.store().revision() == self.before.revision + 3,
                    "Native Redo did not restore the complete grouped scene",
                )?;
                require(
                    proof::same_world(self, &fixture::entities(editor)?),
                    "Redo changed a grouped world pose",
                )?;
                self.pass();
            }
        }
        Ok(())
    }

    pub(super) fn context(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            1 => {
                self.click_at(
                    window,
                    row(shell, OTHER),
                    scale,
                    true,
                    "unselected hierarchy context row",
                    events,
                )?;
            }
            2 => {
                self.click_at(
                    window,
                    command(shell, "create.group", false),
                    scale,
                    false,
                    "Entity > Group Selection",
                    events,
                )?;
            }
            3 => {
                let entities = fixture::entities(editor)?;
                let id = editor
                    .state
                    .selection
                    .first()
                    .cloned()
                    .ok_or("Context Group did not select a container")?;
                require(
                    self.before.records.get(&id).is_none()
                        && proof::children(&entities, &id) == [OTHER],
                    "Context Group affected the unrelated selected set instead of its clicked row",
                )?;
                let after = fixture::records(editor)?;
                require(
                    after[FIRST] == self.before.records[FIRST]
                        && after[LINKED] == self.before.records[LINKED],
                    "Context Group changed unrelated selected sources",
                )?;
                require(
                    editor.host.store().revision() == self.before.revision + 1
                        && proof::same_world(self, &entities),
                    "Context Group was not one pose-preserving content action",
                )?;
                self.group = Some(id.clone());
                self.mark("interaction",editor,shell,json!({"clicked":OTHER,"previousSelection":self.before.selection,"group":id,"unrelatedSourcesPreserved":true}));
                self.next();
            }
            4 => {
                let id = self.group.as_deref().ok_or("Context group ID missing")?;
                self.click_at(
                    window,
                    row(shell, id),
                    scale,
                    true,
                    "new group context",
                    events,
                )?;
            }
            5 => {
                self.click_at(
                    window,
                    submenu(shell, "Create Child"),
                    scale,
                    false,
                    "Entity > Create Child",
                    events,
                )?;
            }
            6 => {
                self.click_at(
                    window,
                    command(shell, "create.empty_child", false),
                    scale,
                    false,
                    "Create Child > Empty Child",
                    events,
                )?;
            }
            7 => {
                let child = editor
                    .state
                    .selection
                    .first()
                    .cloned()
                    .ok_or("Empty Child was not selected")?;
                let entities = fixture::entities(editor)?;
                require(
                    entities
                        .get(&child)
                        .is_some_and(|entity| entity.parent.as_deref() == self.group.as_deref()),
                    "Native Create Child used a different parent",
                )?;
                require(
                    editor.host.store().revision() == self.before.revision + 2,
                    "Native Create Child did not make exactly one action",
                )?;
                self.child = Some(child);
                self.next();
            }
            8 | 12 => self.key(window, 59, false, events)?, // F2
            9 => {
                self.replace(
                    shell,
                    window,
                    "entity-rename-value",
                    "Native child renamed",
                    scale,
                    events,
                )?;
            }
            10 => self.key(window, 40, false, events)?,
            11 => {
                let after = fixture::records(editor)?;
                let id = self.child.as_deref().ok_or("Renamed child ID missing")?;
                require(
                    after[id]["name"] == "Native child renamed"
                        && editor.host.store().revision() == self.before.revision + 3,
                    "F2/Enter did not commit exactly one rename",
                )?;
                self.mark(
                    "renamed",
                    editor,
                    shell,
                    json!({"entity":id,"name":"Native child renamed"}),
                );
                self.next();
            }
            13 => {
                self.replace(
                    shell,
                    window,
                    "entity-rename-value",
                    "Discarded native rename",
                    scale,
                    events,
                )?;
            }
            14 => self.key(window, 41, false, events)?,
            _ => {
                let after = fixture::records(editor)?;
                let id = self
                    .child
                    .as_deref()
                    .ok_or("Rename cancellation child missing")?;
                require(
                    after[id]["name"] == "Native child renamed"
                        && editor.host.store().revision() == self.before.revision + 3,
                    "Escape committed a cancelled entity rename",
                )?;
                require(
                    after[FIRST] == self.before.records[FIRST]
                        && after[LINKED] == self.before.records[LINKED],
                    "Context creation/rename changed unrelated selected sources",
                )?;
                require(
                    !shell.modal_active() && shell.input.captured().is_none(),
                    "Context or inline rename left input captured",
                )?;
                self.pass();
            }
        }
        Ok(())
    }
}
