//! Migration repairs typed references and known path owners without changing labels.
use serde_json::Value;

#[derive(Clone, Copy)]
pub(super) enum Owner {
    Document,
    Project,
    Resources,
    Export,
}

pub(super) fn path(text: &str, mappings: &[(String, String)]) -> String {
    let (prefix, path) = text
        .strip_prefix('@')
        .map_or(("", text), |path| ("@", path));
    for (from, to) in mappings {
        if (path == from && !to.is_empty())
            || path
                .strip_prefix(from)
                .is_some_and(|tail| tail.starts_with('/') || tail.starts_with('#'))
        {
            let tail = &path[from.len()..];
            return format!(
                "{prefix}{to}{}",
                if to.is_empty() {
                    tail.strip_prefix('/').unwrap_or(tail)
                } else {
                    tail
                }
            );
        }
    }
    text.to_owned()
}

pub(super) fn text(text: &str, mappings: &[(String, String)], explicit_only: bool) -> String {
    let repair = |token: &str| {
        if !explicit_only || token.starts_with('@') {
            path(token, mappings)
        } else {
            token.to_owned()
        }
    };
    let mut output = String::with_capacity(text.len());
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if matches!(
            character,
            '"' | '\'' | '`' | '<' | '>' | '\n' | '\r' | '\t' | ' ' | '(' | ')'
        ) {
            output.push_str(&repair(&text[start..index]));
            output.push(character);
            start = index + character.len_utf8();
        }
    }
    output.push_str(&repair(&text[start..]));
    output
}

pub(super) fn references(value: &mut Value, mappings: &[(String, String)]) {
    match value {
        Value::String(value) => *value = text(value, mappings, true),
        Value::Array(items) => items.iter_mut().for_each(|item| references(item, mappings)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|item| references(item, mappings)),
        _ => {}
    }
}

pub(super) fn document(value: &mut Value, mappings: &[(String, String)], owner: Owner) {
    references(value, mappings);
    let pointers: &[&str] = match owner {
        Owner::Document => &[],
        Owner::Project => &[
            "/main",
            "/look/default",
            "/ui/main",
            "/ui/stylesheet",
            "/ui/bindings",
            "/ui/resources",
        ],
        Owner::Resources => &["/click"],
        Owner::Export => &["/icon", "/scriptProject"],
    };
    for pointer in pointers {
        if let Some(value) = value.pointer_mut(pointer) {
            bare(value, mappings);
        }
    }
    if matches!(owner, Owner::Resources) {
        if let Some(items) = value.get_mut("fonts").and_then(Value::as_array_mut) {
            items.iter_mut().for_each(|item| bare(item, mappings));
        }
        if let Some(items) = value.get_mut("images").and_then(Value::as_object_mut) {
            items.values_mut().for_each(|item| bare(item, mappings));
        }
    }
    if matches!(owner, Owner::Export)
        && let Some(items) = value.get_mut("templates").and_then(Value::as_array_mut)
    {
        for item in items {
            if let Some(source) = item.get_mut("source") {
                bare(source, mappings);
            }
        }
    }
}

fn bare(value: &mut Value, mappings: &[(String, String)]) {
    if let Value::String(value) = value {
        *value = path(value, mappings);
    }
}

pub(super) fn policy_history(value: &mut Value, field: &str, mappings: &[(String, String)]) {
    match field {
        "settings" => document(value, mappings, Owner::Project),
        "settings.main"
        | "settings.look.default"
        | "settings.ui.main"
        | "settings.ui.stylesheet"
        | "settings.ui.bindings"
        | "settings.ui.resources" => bare(value, mappings),
        "settings.look" | "settings.ui" => {
            let key = field
                .strip_prefix("settings.")
                .expect("matched policy field");
            let mut wrapped = serde_json::json!({key: value.clone()});
            document(&mut wrapped, mappings, Owner::Project);
            *value = wrapped[key].take();
        }
        _ => {}
    }
}
