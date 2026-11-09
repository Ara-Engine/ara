//! Collection rows with one selected item's typed details. Short collections (every
//! row fits the window) open that item in place, under its own row, like an
//! accordion; longer ones keep a bounded row window with the details below it,
//! outside its recycled row pool.
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
        // Every row is mounted: no recycling, so details can follow their row.
        let inline = window.range.len() == count;
        // Items that open into fields read as cards.
        let cards = inline
            && match ty {
                ValueType::List(element) | ValueType::Keyed(element) | ValueType::Map(element) => {
                    crate::inspector::compound(editor.host.store().schema(), inner(element))
                }
                _ => false,
            };
        if inline {
            out.push_str(&format!("<panel id='{scroll}' class='collection-inline'>"));
        } else {
            out.push_str(&format!("<scroll id='{scroll}' name='collection' style='height:{height}px;min-height:{height}px;flex-shrink:0;gap:0px;overflow:hidden'><panel id='{scroll}-before' style='height:{}px;min-height:{}px;flex-shrink:0' />",window.before,window.before));
        }
        let mut detail = None;
        match ty {
            ValueType::List(element) | ValueType::Keyed(element) => {
                if let Some(items) = value.as_array() {
                    for index in window.range.clone() {
                        let item = &items[index];
                        let item_pointer = format!("{pointer}/{index}");
                        // Reorder by dragging the grip; Remove stays one click away.
                        let actions = vec![(
                            "remove",
                            "x",
                            "Remove",
                            edit(editor, field, &item_pointer, Operation::Remove),
                        )];
                        let key = item_key(ty, item, index);
                        let select = CollectionAction {
                            field: field.path.clone(),
                            pointer: pointer.into(),
                            intent: Intent::Select(key.clone()),
                            revision: editor.host.store().revision(),
                        };
                        if cards {
                            out.push_str(&format!(
                                "<panel id='{scroll}-card-{index}' class='collection-card'>"
                            ));
                        }
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
                        if selected.as_ref() == Some(&key) {
                            detail = Some((element.as_ref(), item, item_pointer));
                            if inline {
                                self.collection_detail(
                                    editor,
                                    field,
                                    detail.take(),
                                    // The card already groups its fields.
                                    depth.saturating_sub(1),
                                    label,
                                    &scroll,
                                    out,
                                );
                            }
                        }
                        if cards {
                            out.push_str("</panel>");
                        }
                    }
                }
            }
            ValueType::Map(element) => {
                if let Some(map) = value.as_object() {
                    for (n, (key, item)) in map
                        .iter()
                        .enumerate()
                        .skip(window.range.start)
                        .take(window.range.len())
                    {
                        let item_pointer = format!("{pointer}/{}", token(key));
                        if cards {
                            out.push_str(&format!(
                                "<panel id='{scroll}-card-{n}' class='collection-card'>"
                            ));
                        }
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
                        if selected.as_ref() == Some(&ItemKey::Key(key.clone())) {
                            detail = Some((element.as_ref(), item, item_pointer));
                            if inline {
                                self.collection_detail(
                                    editor,
                                    field,
                                    detail.take(),
                                    // The card already groups its fields.
                                    depth.saturating_sub(1),
                                    label,
                                    &scroll,
                                    out,
                                );
                            }
                        }
                        if cards {
                            out.push_str("</panel>");
                        }
                    }
                }
            }
            _ => {}
        }
        if inline {
            out.push_str("</panel>");
        } else {
            out.push_str(&format!("<panel id='{scroll}-after' style='height:{}px;min-height:{}px;flex-shrink:0' /></scroll>",window.after,window.after));
            // The selected item may be scrolled out of the window; look it up.
            let found = match (ty, value, &selected) {
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
            self.collection_detail(editor, field, found.or(detail), depth, label, &scroll, out);
        }
    }

    /// The selected item's own rows, one level deeper than its row.
    #[allow(clippy::too_many_arguments)]
    fn collection_detail(
        &mut self,
        editor: &Editor,
        field: &Field,
        detail: Option<(&ValueType, &Value, String)>,
        depth: usize,
        label: f32,
        scroll: &str,
        out: &mut String,
    ) {
        let Some((element, item, item_pointer)) = detail else {
            return;
        };
        if !crate::inspector::compound(editor.host.store().schema(), inner(element)) {
            return;
        }
        out.push_str(&format!(
            "<panel id='{scroll}-detail' class='collection-detail'>"
        ));
        // A module-like item's kind is its header title, so it is not repeated.
        let db = editor.host.store().schema();
        let title = leading_enum(db, element).map(|(name, _)| name);
        self.items_except(
            editor,
            field,
            element,
            item,
            &item_pointer,
            depth + 1,
            label,
            title,
            out,
        );
        out.push_str("</panel>");
    }
}
