//! A bounded row window with one selected item's typed details outside its row pool.
use super::*;

fn item_key(ty: &ValueType, item: &Value, index: usize) -> ItemKey {
    if matches!(ty, ValueType::Keyed(_))
        && let Some(id) = item.get("id").and_then(Value::as_str)
    {
        ItemKey::Key(id.into())
    } else {
        ItemKey::Index(index)
    }
}

fn edit(editor: &Editor, field: &Field, pointer: &str, operation: Operation) -> CollectionAction {
    CollectionAction {
        field: field.path.clone(),
        pointer: pointer.into(),
        intent: Intent::Edit {
            operation,
            value: Value::Null,
            index: None,
        },
        revision: editor.host.store().revision(),
    }
}

impl Shell {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn collection_items(
        &mut self,
        editor: &Editor,
        field: &Field,
        ty: &ValueType,
        value: &Value,
        pointer: &str,
        depth: usize,
        label: f32,
        out: &mut String,
    ) {
        let count = value
            .as_array()
            .map(Vec::len)
            .or_else(|| value.as_object().map(|map| map.len()))
            .unwrap_or(0);
        let (scroll, window, selected, height) =
            self.collection_window(editor, &field.path, pointer, count);
        let selected = match (value, &selected) {
            (Value::Array(items), Some(ItemKey::Index(index))) => {
                items.get(*index).map(|item| item_key(ty, item, *index))
            }
            (Value::Array(items), Some(ItemKey::Key(key))) => items
                .iter()
                .any(|item| item.get("id").and_then(Value::as_str) == Some(key))
                .then(|| ItemKey::Key(key.clone())),
            (Value::Object(map), Some(ItemKey::Index(index))) => {
                map.keys().nth(*index).map(|key| ItemKey::Key(key.clone()))
            }
            (Value::Object(map), Some(ItemKey::Key(key))) => {
                map.contains_key(key).then(|| ItemKey::Key(key.clone()))
            }
            _ => None,
        };
        if let Some(view) = self.collections.get_mut(&scroll) {
            view.selected = selected.clone();
        }
        out.push_str(&format!("<scroll id='{scroll}' name='collection' style='height:{height}px;min-height:{height}px;flex-shrink:0;gap:0px;overflow:hidden'><panel id='{scroll}-before' style='height:{}px;min-height:{}px;flex-shrink:0' />",window.before,window.before));
        match ty {
            ValueType::List(element) | ValueType::Keyed(element) => {
                if let Some(items) = value.as_array() {
                    for index in window.range.clone() {
                        let item = &items[index];
                        let item_pointer = format!("{pointer}/{index}");
                        let mut actions = Vec::new();
                        if index > 0 {
                            actions.push((
                                "up",
                                "chevron-up",
                                "Move up",
                                edit(editor, field, &item_pointer, Operation::MoveUp),
                            ));
                        }
                        if index + 1 < items.len() {
                            actions.push((
                                "down",
                                "chevron-down",
                                "Move down",
                                edit(editor, field, &item_pointer, Operation::MoveDown),
                            ));
                        }
                        actions.push((
                            "remove",
                            "x",
                            "Remove",
                            edit(editor, field, &item_pointer, Operation::Remove),
                        ));
                        let select = CollectionAction {
                            field: field.path.clone(),
                            pointer: pointer.into(),
                            intent: Intent::Select(item_key(ty, item, index)),
                            revision: editor.host.store().revision(),
                        };
                        self.element(
                            editor,
                            field,
                            Row {
                                title: Title::Text(item_title(
                                    editor.host.store().schema(),
                                    element,
                                    item,
                                    index,
                                )),
                                ty: element,
                                desc: None,
                                value: item,
                                pointer: &item_pointer,
                                actions,
                                item: true,
                                details: false,
                                select: Some(select),
                            },
                            depth,
                            label,
                            out,
                        );
                    }
                }
            }
            ValueType::Map(element) => {
                if let Some(map) = value.as_object() {
                    for (key, item) in map.iter().skip(window.range.start).take(window.range.len())
                    {
                        let item_pointer = format!("{pointer}/{}", token(key));
                        let select = CollectionAction {
                            field: field.path.clone(),
                            pointer: pointer.into(),
                            intent: Intent::Select(ItemKey::Key(key.clone())),
                            revision: editor.host.store().revision(),
                        };
                        self.element(
                            editor,
                            field,
                            Row {
                                title: Title::Key(key.clone()),
                                ty: element,
                                desc: None,
                                value: item,
                                pointer: &item_pointer,
                                actions: vec![(
                                    "remove",
                                    "x",
                                    "Remove",
                                    edit(editor, field, &item_pointer, Operation::Remove),
                                )],
                                item: true,
                                details: false,
                                select: Some(select),
                            },
                            depth,
                            label,
                            out,
                        );
                    }
                }
            }
            _ => {}
        }
        out.push_str(&format!("<panel id='{scroll}-after' style='height:{}px;min-height:{}px;flex-shrink:0' /></scroll>",window.after,window.after));
        let detail = match (ty, value, &selected) {
            (
                ValueType::List(element) | ValueType::Keyed(element),
                Value::Array(items),
                Some(key),
            ) => {
                let index = match key {
                    ItemKey::Index(index) => Some(*index),
                    ItemKey::Key(key) => items
                        .iter()
                        .position(|item| item.get("id").and_then(Value::as_str) == Some(key)),
                };
                index.and_then(|index| {
                    items
                        .get(index)
                        .map(|item| (element.as_ref(), item, format!("{pointer}/{index}")))
                })
            }
            (ValueType::Map(element), Value::Object(map), Some(ItemKey::Key(key))) => map
                .get(key)
                .map(|item| (element.as_ref(), item, format!("{pointer}/{}", token(key)))),
            _ => None,
        };
        if let Some((element, item, item_pointer)) = detail
            && crate::inspector::compound(editor.host.store().schema(), inner(element))
        {
            out.push_str(&format!("<panel id='{scroll}-detail' style='flex-shrink:0;gap:0px;border-width:1px;border-color:var(--border)'>"));
            self.items(
                editor,
                field,
                element,
                item,
                &item_pointer,
                depth + 1,
                label,
                out,
            );
            out.push_str("</panel>");
        }
    }
}
