//! Inspector collection windows and typed action lowering; no UI-side document writes.
use super::*;
use crate::commands::nested::{self, Operation};
use ara_ui::collection::{ItemKey, Viewport};
use std::ops::Range;
#[cfg(test)]
#[path = "collection_adapter/append_tests.rs"]
mod append_tests;

/// Only the primary object's presentation; mutations resolve every object afresh.
pub(crate) struct Values {
    pub(super) value: Value,
}

/// Retained scroll window and selected stable item, independently of recycled rows.
pub(crate) struct View {
    pub(super) range: Range<usize>,
    pub(super) selected: Option<ItemKey>,
    count: usize,
    viewport: Viewport,
    surface: Option<String>,
    /// A successful explicit local Add, consumed after its new rows are mounted.
    reveal: Option<(String, String, usize)>,
}

/// One collection control's typed intent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Action {
    pub(super) field: String,
    pub(super) pointer: String,
    pub(super) intent: Intent,
    /// Positional row drags reject stale captures after an undo or authored edit.
    pub(super) revision: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Intent {
    Select(ItemKey),
    Edit {
        operation: Operation,
        value: Value,
        index: Option<u32>,
    },
}

pub(super) fn id(field: &str, pointer: &str) -> String {
    format!(
        "collection-{:016x}",
        ara_ui::source::stable_id(&format!("{field}#{pointer}"))
    )
}

/// Build commands rather than whole-field snapshots; later calls observe earlier batch ops.
#[allow(clippy::too_many_arguments)]
pub(super) fn commands(
    editor: &Editor,
    selection: &[String],
    field: &Field,
    reference: &Value,
    pointer: &str,
    operation: Operation,
    value: Value,
    index: Option<u32>,
) -> ara_ui::Result<Vec<Value>> {
    let db = editor.host.store().schema();
    let path = field.path.parse::<ara_schema::FieldPath>()?;
    let path = db
        .resolve_path(db.get("Entity").ok_or("Entity schema missing")?, &path)
        .map_err(|error| error.to_string())?;
    let fallback;
    let reference = if reference.is_null() && field.mixed {
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|errors| format!("{errors:?}"))?;
        let primary = selection
            .first()
            .and_then(|entity| resolved.get(entity))
            .ok_or("The selected object no longer exists")?;
        fallback =
            ara_schema::path::get_effective(&primary.record, &path.steps).unwrap_or(Value::Null);
        &fallback
    } else {
        reference
    };
    let hints =
        nested::hints(db, &path.ty, reference, pointer).map_err(|error| error.to_string())?;
    Ok(selection.iter().map(|entity|json!({"cmd":"editor.property.nested.edit","args":{
        "entity":entity,"path":field.path,"pointer":pointer,"hints":hints,"operation":operation,"value":value,"index":index,
    }})).collect())
}

impl Shell {
    pub(super) fn cache_compound(&mut self, editor: &Editor, field: &Field) {
        let value = if field.mixed {
            let found = self.resolved(editor).ok().and_then(|resolved| {
                let entity = editor
                    .state
                    .selection
                    .first()
                    .and_then(|id| resolved.get(id))?;
                let db = editor.host.store().schema();
                let path = field.path.parse::<ara_schema::FieldPath>().ok()?;
                let path = db.resolve_path(db.get("Entity")?, &path).ok()?;
                Some(
                    ara_schema::path::get_effective(&entity.record, &path.steps).unwrap_or_else(
                        || {
                            path.field
                                .map_or(Value::Null, |field| db.field_default(field))
                        },
                    ),
                )
            });
            found.unwrap_or(Value::Null)
        } else {
            field.value.clone()
        };
        self.compound_values
            .insert(field.path.clone(), Values { value });
    }

    /// Request a host rebuild when scrolling crosses a virtual collection row boundary.
    pub(crate) fn sync_collections(&mut self) -> bool {
        let mut changed = false;
        for (id, view) in &mut self.collections {
            if view.surface != self.current || view.reveal.is_some() {
                continue;
            }
            let Some(index) = self.ui.find(id).filter(|index| self.ui.shown(*index)) else {
                continue;
            };
            let offset = self
                .ui
                .scroll_position(index)
                .map_or(0., |position| position[1]);
            let range = view.viewport.window(view.count, offset).range;
            if range != view.range {
                view.range = range;
                changed = true;
            }
        }
        changed
    }

    /// Run a selection or a current-state patch as one human undo transaction.
    pub(crate) fn collection_action(
        &mut self,
        editor: &mut Editor,
        action: &Action,
    ) -> ara_ui::Result<()> {
        match &action.intent {
            Intent::Select(key) => {
                if let Some(view) = self
                    .collections
                    .get_mut(&id(&action.field, &action.pointer))
                {
                    view.selected = if view.selected.as_ref() == Some(key) {
                        None
                    } else {
                        Some(key.clone())
                    };
                }
            }
            Intent::Edit {
                operation,
                value,
                index,
            } => {
                let field = self
                    .fields
                    .get(&action.field)
                    .ok_or("The collection is no longer shown")?;
                if field.read_only {
                    return Err("This collection is read-only".into());
                }
                let reference = self
                    .compound_values
                    .get(&field.path)
                    .map_or(&field.value, |values| &values.value);
                let calls = commands(
                    editor,
                    &editor.state.selection,
                    field,
                    reference,
                    &action.pointer,
                    *operation,
                    value.clone(),
                    *index,
                )?;
                let result = editor.apply(calls, "Edit collection", None)?;
                if *operation == Operation::Append {
                    self.reveal_appended_collection(editor, &action.field, &action.pointer)?;
                }
                let skipped = count_skipped(&result);
                if skipped > 0 {
                    self.status_note = Some(format!(
                        "Updated matching entries; {skipped} object(s) had no matching item and were left unchanged"
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn collection_selected(&self, action: &Action) -> bool {
        let Intent::Select(key) = &action.intent else {
            return false;
        };
        self.collections
            .get(&id(&action.field, &action.pointer))
            .is_some_and(|view| view.selected.as_ref() == Some(key))
    }

    /// Reorder a captured stable row, with an execution-time patch for each object.
    pub(crate) fn collection_reorder(
        &mut self,
        editor: &mut Editor,
        source: &Action,
        target: &Action,
        after: bool,
    ) -> ara_ui::Result<()> {
        if source.field != target.field || source.pointer != target.pointer {
            return Err("Items can only be reordered within the same collection".into());
        }
        let (Intent::Select(source_key), Intent::Select(target_key)) =
            (&source.intent, &target.intent)
        else {
            return Err("Choose two collection rows".into());
        };
        if matches!(source_key, ItemKey::Index(_))
            && source.revision != editor.host.store().revision()
        {
            return Err("This list changed during the drag; choose the item again".into());
        }
        let resolved = self.resolved(editor)?;
        let entity = editor
            .state
            .selection
            .first()
            .and_then(|entity| resolved.get(entity))
            .ok_or("The edited object no longer exists")?;
        let db = editor.host.store().schema();
        let path = source.field.parse::<ara_schema::FieldPath>()?;
        let path = db
            .resolve_path(db.get("Entity").ok_or("Entity schema missing")?, &path)
            .map_err(|error| error.to_string())?;
        let value =
            ara_schema::path::get_effective(&entity.record, &path.steps).unwrap_or(Value::Null);
        let items = value
            .pointer(&source.pointer)
            .and_then(Value::as_array)
            .ok_or("Map entries are ordered by key; only list items can be reordered")?;
        let index = |key: &ItemKey| match key {
            ItemKey::Index(index) => (*index < items.len()).then_some(*index),
            ItemKey::Key(key) => items
                .iter()
                .position(|item| item.get("id").and_then(Value::as_str) == Some(key)),
        };
        let from = index(source_key).ok_or("The dragged item no longer exists")?;
        let to = index(target_key).ok_or("The destination item no longer exists")?;
        let insertion = to + usize::from(after);
        let to = if from < insertion {
            insertion - 1
        } else {
            insertion
        };
        let field = self
            .fields
            .get(&source.field)
            .ok_or("The collection is no longer shown")?;
        let relative = match target_key {
            ItemKey::Key(key) => json!({"key":key,"after":after}),
            ItemKey::Index(_) => Value::Null,
        };
        let calls = commands(
            editor,
            &editor.state.selection,
            field,
            &value,
            &format!("{}/{from}", source.pointer),
            Operation::Move,
            relative,
            Some(to as u32),
        )?;
        let result = editor.apply(calls, "Reorder collection", None)?;
        let skipped = count_skipped(&result);
        if skipped > 0 {
            self.status_note = Some(format!(
                "Reordered matching entries; {skipped} object(s) had no matching item"
            ));
        }
        Ok(())
    }

    pub(super) fn collection_window(
        &mut self,
        editor: &Editor,
        field: &str,
        pointer: &str,
        count: usize,
    ) -> (String, ara_ui::collection::Window, Option<ItemKey>, f32) {
        let id = id(field, pointer);
        let row = if editor.state.comfortable() { 28. } else { 24. };
        let height = (count as f32 * row).clamp(row, 10. * row);
        let viewport = Viewport::new(row, height)
            .expect("collection rows and viewport have positive fixed metrics");
        let offset = self
            .ui
            .find(&id)
            .and_then(|index| self.ui.scroll_position(index))
            .map_or(0., |position| position[1]);
        if let Some(view) = self.collections.get_mut(&id)
            && view.reveal.as_ref().is_some_and(|(doc, entity, _)| {
                doc != &editor.state.doc || editor.state.selection.first() != Some(entity)
            })
        {
            view.reveal = None;
        }
        let requested = self
            .collections
            .get(&id)
            .and_then(|view| view.reveal.as_ref().map(|(_, _, index)| *index))
            .and_then(|index| viewport.reveal(index, count, offset));
        let window = viewport.window(count, requested.unwrap_or(offset));
        let view = self.collections.entry(id.clone()).or_insert_with(|| View {
            range: window.range.clone(),
            selected: (count > 0).then_some(ItemKey::Index(0)),
            count,
            viewport,
            surface: self.current.clone(),
            reveal: None,
        });
        view.range = window.range.clone();
        view.count = count;
        view.viewport = viewport;
        view.surface = self.current.clone();
        (id, window, view.selected.clone(), height)
    }

    /// Reveal only the item created by an explicit successful Add. Generic model
    /// notifications never call this path or change the collection's selection.
    pub(super) fn reveal_appended_collection(
        &mut self,
        editor: &Editor,
        field: &str,
        pointer: &str,
    ) -> ara_ui::Result<bool> {
        let resolved = self.resolved(editor)?;
        let Some(primary) = editor
            .state
            .selection
            .first()
            .and_then(|entity| resolved.get(entity))
        else {
            return Ok(false);
        };
        let db = editor.host.store().schema();
        let path = field.parse::<ara_schema::FieldPath>()?;
        let path = db
            .resolve_path(db.get("Entity").ok_or("Entity schema missing")?, &path)
            .map_err(|error| error.to_string())?;
        let value =
            ara_schema::path::get_effective(&primary.record, &path.steps).unwrap_or_else(|| {
                path.field
                    .map_or(Value::Null, |field| db.field_default(field))
            });
        let Some(collection) = value.pointer(pointer) else {
            return Ok(false);
        };
        let found = match collection {
            Value::Array(items) => items.last().map(|item| {
                let index = items.len() - 1;
                let key = if matches!(
                    compound::nested_type(db, &path.ty, pointer),
                    Some(ara_schema::ValueType::Keyed(_))
                ) {
                    item.get("id")
                        .and_then(Value::as_str)
                        .map(|key| ItemKey::Key(key.into()))
                        .unwrap_or(ItemKey::Index(index))
                } else {
                    ItemKey::Index(index)
                };
                (index, key, items.len())
            }),
            Value::Object(items) => {
                let before = self
                    .compound_values
                    .get(field)
                    .and_then(|values| values.value.pointer(pointer))
                    .and_then(Value::as_object);
                let mut added = items
                    .keys()
                    .enumerate()
                    .filter(|(_, key)| before.is_none_or(|before| !before.contains_key(*key)));
                let first = added.next();
                if added.next().is_some() {
                    None
                } else {
                    first.map(|(index, key)| (index, ItemKey::Key(key.clone()), items.len()))
                }
            }
            _ => None,
        };
        let Some((index, key, count)) = found else {
            return Ok(false);
        };
        let Some(view) = self.collections.get_mut(&id(field, pointer)) else {
            return Ok(false);
        };
        view.selected = Some(key);
        view.count = count;
        view.reveal = editor
            .state
            .selection
            .first()
            .map(|entity| (editor.state.doc.clone(), entity.clone(), index));
        Ok(true)
    }

    /// Run after mounting and popup-scope synchronization on each surface. The
    /// requested pool is already bounded; native scroll now has the new extent.
    pub(super) fn reveal_collection_appends(&mut self, editor: &Editor) -> ara_ui::Result<()> {
        for view in self.collections.values_mut() {
            if view.reveal.as_ref().is_some_and(|(doc, entity, _)| {
                doc != &editor.state.doc || editor.state.selection.first() != Some(entity)
            }) {
                view.reveal = None;
            }
        }
        let pending: Vec<_> = self
            .collections
            .iter()
            .filter(|(_, view)| view.surface == self.current && view.reveal.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in pending {
            let Some(container) = self.shown_node(&id) else {
                if let Some(view) = self.collections.get_mut(&id) {
                    view.reveal = None;
                }
                continue;
            };
            let view = &self.collections[&id];
            let offset = self
                .ui
                .scroll_position(container)
                .map_or(0., |offset| offset[1]);
            let Some(target) = view
                .reveal
                .as_ref()
                .and_then(|(_, _, index)| view.viewport.reveal(*index, view.count, offset))
            else {
                continue;
            };
            self.ui.scroll_to(container, [0., target])?;
            self.collections
                .get_mut(&id)
                .expect("pending collection exists")
                .reveal = None;
            let Some(detail) = self.shown_node(&format!("{id}-detail")) else {
                continue;
            };
            let editable = self.ui.nodes.iter().enumerate().find_map(|(index, node)| {
                if node.state.disabled
                    || !matches!(
                        node.element.tag.as_str(),
                        "field" | "input" | "dropdown" | "toggle"
                    )
                {
                    return None;
                }
                let mut ancestor = node.element.parent;
                while let Some(parent) = ancestor {
                    if parent == detail {
                        return Some(index);
                    }
                    ancestor = self.ui.nodes[parent].element.parent;
                }
                None
            });
            if let Some(editable) = editable {
                self.ui.scroll_into_view(editable)?;
                self.stats.work += self.ui.frame(&mut self.text)?;
                let focus = ara_ui::source::stable_id(&self.ui.nodes[editable].element.id);
                self.input.focus(&mut self.ui, focus)?;
            }
        }
        Ok(())
    }
}

fn count_skipped(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.get("skipped") == Some(&Value::Bool(true)))
                + map.values().map(count_skipped).sum::<usize>()
        }
        Value::Array(values) => values.iter().map(count_skipped).sum(),
        _ => 0,
    }
}
