//! Reference repair shares the workspace's asset identity map, including addressable records.
use super::{SourceDoc, inside, selected};
use ara_doc::{AssetId, Document, Edit, Record, RecordId, Workspace};
use ara_schema::{FieldPath, grammar::parse_ref};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Canonical JSON file owners whose documented fields contain bare authored paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonOwner {
    /// Authored JSON with explicit references/IDs; bare strings are never inferred as paths.
    Generic,
    /// `ara.project.json`: the startup scene is `main`.
    Marker,
    /// Project policy: startup scene, default look and explicit UI bootstrap paths.
    Policy,
    /// `ui/resources.json`: fonts, image-map values and click audio are file paths.
    UiResources,
    /// `ui/bindings.json`: entity identities are remapped in the owning scene context.
    UiBindings,
    /// `settings/builds/*.export.json`: icon and scriptProject are authored-relative.
    ExportPreset,
}

#[derive(Clone, Copy)]
enum Owner {
    Generic,
    MarkerRecord,
    PolicyRecord,
    Json(JsonOwner),
}
impl Owner {
    fn document(document: &Document) -> Self {
        if document.kind != "project" {
            Self::Generic
        } else if document.id.as_str() == "pj_project" {
            Self::MarkerRecord
        } else {
            Self::PolicyRecord
        }
    }
    fn bare_path(self, path: &[String]) -> bool {
        let fields: Vec<_> = path.iter().map(String::as_str).collect();
        match self {
            Self::Generic => false,
            Self::MarkerRecord => matches!(
                fields.as_slice(),
                ["settings", "main"]
                    | ["settings", "look", "default"]
                    | [
                        "settings",
                        "ui",
                        "main" | "stylesheet" | "bindings" | "resources"
                    ]
            ),
            Self::PolicyRecord => matches!(
                fields.as_slice(),
                ["settings", "main"]
                    | ["settings", "look", "default"]
                    | [
                        "settings",
                        "ui",
                        "main" | "stylesheet" | "bindings" | "resources"
                    ]
            ),
            Self::Json(JsonOwner::Marker) => matches!(fields.as_slice(), ["main"]),
            Self::Json(JsonOwner::Policy) => matches!(
                fields.as_slice(),
                ["main"]
                    | ["look", "default"]
                    | ["ui", "main" | "stylesheet" | "bindings" | "resources"]
            ),
            Self::Json(JsonOwner::ExportPreset) => {
                matches!(fields.as_slice(), ["icon"] | ["scriptProject"])
            }
            Self::Json(JsonOwner::UiResources) => {
                matches!(fields.as_slice(), ["fonts", _] | ["images", _] | ["click"])
            }
            Self::Json(JsonOwner::UiBindings | JsonOwner::Generic) => false,
        }
    }
}

/// Rewrites references and the owner's explicitly defined authored path fields.
/// Host preparation uses this pure helper to prepare canonical replacement blobs;
/// the normal file-state transaction remains responsible for persistence/undo.
pub fn rewrite_json_paths(owner: JsonOwner, source: &str, target: &str, value: &mut Value) -> bool {
    let before = value.clone();
    let mappings = Mappings::new(source, Some(target), &[]);
    mappings.value(value, "", Owner::Json(owner), &mut Vec::new());
    *value != before
}

pub(super) fn asset_path<'a>(path: &'a str, kind: &str) -> &'a str {
    if kind == "import" {
        path.strip_suffix(".import.json").unwrap_or(path)
    } else {
        path
    }
}
pub(super) struct Mappings {
    source: String,
    target: Option<String>,
    pub assets: BTreeMap<String, (AssetId, String)>,
    pub records: BTreeMap<(String, String), RecordId>,
    paths: BTreeMap<String, String>,
    originals: BTreeMap<String, Document>,
    path_moves: BTreeMap<String, String>,
}
impl Mappings {
    pub fn new(source: &str, target: Option<&str>, documents: &[SourceDoc]) -> Self {
        let mut assets = BTreeMap::new();
        let mut paths = BTreeMap::new();
        let mut originals = BTreeMap::new();
        for source_doc in documents {
            let document = &source_doc.document;
            let path = asset_path(&source_doc.reference_path, &document.kind);
            let target_path = target.map_or_else(
                || path.to_owned(),
                |target| format!("{target}{}", &path[source.len()..]),
            );
            assets.insert(document.id.to_string(), (document.id.clone(), target_path));
            paths.insert(path.to_owned(), document.id.to_string());
            originals.insert(document.id.to_string(), document.clone());
        }
        Self {
            source: source.to_owned(),
            target: target.map(str::to_owned),
            assets,
            records: BTreeMap::new(),
            paths,
            originals,
            path_moves: BTreeMap::new(),
        }
    }
    pub(super) fn relocate(&mut self, source: &str, target: &str) {
        self.path_moves.insert(source.to_owned(), target.to_owned());
    }
    pub(super) fn rewrite_json(
        &self,
        owner: JsonOwner,
        context: Option<&AssetId>,
        value: &mut Value,
    ) {
        self.value(
            value,
            context.map_or("", AssetId::as_str),
            Owner::Json(owner),
            &mut Vec::new(),
        );
    }
    fn document_for_ref(&self, text: &str) -> Option<&str> {
        let parts = parse_ref(text).ok()?;
        parts
            .id
            .and_then(|id| self.originals.get_key_value(id).map(|(id, _)| id.as_str()))
            .or_else(|| {
                parts
                    .path
                    .and_then(|path| self.paths.get(path).map(String::as_str))
            })
    }
    fn path(&self, path: &str) -> String {
        if let Some((source, target)) = self
            .path_moves
            .iter()
            .filter(|(source, _)| selected(path, source))
            .max_by_key(|(source, _)| source.len())
        {
            return format!("{target}{}", &path[source.len()..]);
        }
        match &self.target {
            Some(target) if selected(path, &self.source) => {
                format!("{target}{}", &path[self.source.len()..])
            }
            _ => path.to_owned(),
        }
    }
    fn address(&self, text: &str, context: &str) -> String {
        let mut context = context;
        let mut output = Vec::new();
        for segment in text.split('/') {
            let mapped = self.records.get(&(context.to_owned(), segment.to_owned()));
            output.push(mapped.map_or_else(|| segment.to_owned(), ToString::to_string));
            if let Some(prefab) = self
                .originals
                .get(context)
                .and_then(|doc| doc.record(segment))
                .and_then(|record| record.get("prefab"))
                .and_then(Value::as_str)
            {
                // An external prefab keeps its own inner record identities.
                context = self.document_for_ref(prefab).unwrap_or("");
            }
        }
        output.join("/")
    }
    fn scalar(&self, text: &str, context: &str) -> String {
        if text.starts_with('@')
            && let Ok(parts) = parse_ref(text)
        {
            if let Some((id, path)) = parts.id.and_then(|id| self.assets.get(id)) {
                return format!("@{path}#{id}");
            }
            if let Some(path) = parts.path {
                let new = self
                    .paths
                    .get(path)
                    .and_then(|source| self.assets.get(source))
                    .map_or_else(|| self.path(path), |(_, path)| path.clone());
                let id = parts.id.map(|id| {
                    let scope = self.paths.get(path).map_or(context, String::as_str);
                    self.records
                        .get(&(scope.to_owned(), id.to_owned()))
                        .map_or_else(|| id.to_owned(), ToString::to_string)
                });
                return id.map_or_else(|| format!("@{new}"), |id| format!("@{new}#{id}"));
            }
        }
        if let Some((id, _)) = self.assets.get(text) {
            return id.to_string();
        }
        self.address(text, context)
    }
    fn text(&self, text: &str, context: &str) -> String {
        let direct = self.scalar(text, context);
        if direct != text {
            return direct;
        }
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;
        while let Some(offset) = text[cursor..].find('@') {
            let start = cursor + offset;
            output.push_str(&self.tokens(&text[cursor..start], context));
            let remaining = &text[start..];
            let boundary = remaining
                .char_indices()
                .skip(1)
                .find(|(_, c)| {
                    matches!(
                        c,
                        '"' | '\'' | '<' | '>' | '\n' | '\r' | '(' | ')' | '}' | '{'
                    )
                })
                .map_or(remaining.len(), |(i, _)| i);
            let candidate = &remaining[..boundary];
            let length = if let Some(hash) = candidate.find('#') {
                hash + 1
                    + candidate[hash + 1..]
                        .chars()
                        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                        .count()
            } else {
                candidate
                    .find(char::is_whitespace)
                    .unwrap_or(candidate.len())
            };
            let length = length.max(1);
            output.push_str(&self.scalar(&remaining[..length], context));
            cursor = start + length;
        }
        output.push_str(&self.tokens(&text[cursor..], context));
        output
    }
    fn tokens(&self, text: &str, context: &str) -> String {
        let mut output = String::new();
        let mut start = 0;
        for (index, character) in text.char_indices() {
            if matches!(
                character,
                '"' | '\''
                    | '<'
                    | '>'
                    | '\n'
                    | '\r'
                    | '\t'
                    | ' '
                    | '('
                    | ')'
                    | '}'
                    | '{'
                    | '='
                    | ':'
            ) {
                output.push_str(&self.scalar(&text[start..index], context));
                output.push(character);
                start = index + character.len_utf8();
            }
        }
        output.push_str(&self.scalar(&text[start..], context));
        output
    }
    fn value(&self, value: &mut Value, context: &str, owner: Owner, path: &mut Vec<String>) {
        match value {
            Value::String(text) => {
                *text = if owner.bare_path(path) {
                    self.path(&self.text(text, context))
                } else {
                    self.text(text, context)
                }
            }
            Value::Array(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    path.push(index.to_string());
                    self.value(value, context, owner, path);
                    path.pop();
                }
            }
            Value::Object(fields) => {
                let old = std::mem::take(fields);
                for (key, mut value) in old {
                    path.push(key.clone());
                    self.value(&mut value, context, owner, path);
                    path.pop();
                    fields.insert(self.scalar(&key, context), value);
                }
            }
            _ => {}
        }
    }
    pub fn rewrite_record(&self, document: &Document, record: &mut Record) {
        let context = document.id.as_str();
        let owner = Owner::document(document);
        let prefab = record
            .get("prefab")
            .and_then(Value::as_str)
            .and_then(|text| self.document_for_ref(text))
            .map(str::to_owned);
        for (key, value) in record.iter_mut() {
            if key == "overrides"
                && let Some(overrides) = value.as_object_mut()
            {
                let old = std::mem::take(overrides);
                for (path, mut value) in old {
                    let scope = prefab.as_deref().unwrap_or("");
                    self.value(&mut value, scope, Owner::Generic, &mut Vec::new());
                    overrides.insert(self.address(&path, scope), value);
                }
            } else if key == "removed" {
                self.value(
                    value,
                    prefab.as_deref().unwrap_or(""),
                    Owner::Generic,
                    &mut Vec::new(),
                );
            } else {
                self.value(value, context, owner, &mut vec![key.clone()]);
            }
        }
        // Only the selected UI changes storage/identity. Inbound UI consumers keep
        // their original bytes apart from the references repaired above.
        if document.kind == "ui"
            && let Some((id, _)) = self.assets.get(context)
            && let Some(Value::String(source)) = record.get_mut("source")
        {
            pin_ui_identity(source, id);
        }
    }
}

fn pin_ui_identity(source: &mut String, id: &AssetId) {
    const HEADER: &str = "<!-- ara:asset ";
    if source.lines().any(|line| {
        line.trim()
            .strip_prefix(HEADER)
            .and_then(|metadata| metadata.strip_suffix(" -->"))
            .is_some()
    }) {
        return;
    }
    if source.starts_with("<?xml")
        && let Some(end) = source.find("?>")
    {
        let end = end + 2;
        *source = format!("{}\n{HEADER}{id} -->\n{}", &source[..end], &source[end..]);
    } else {
        source.insert_str(0, &format!("{HEADER}{id} -->\n"));
    }
}

pub(super) fn fixups(workspace: &Workspace, mappings: &Mappings) -> Vec<Edit> {
    let mut edits = Vec::new();
    for (_, document) in workspace.docs() {
        for (id, record) in document.records() {
            let mut changed = record.clone();
            mappings.rewrite_record(document, &mut changed);
            for (key, value) in changed {
                if record.get(&key) != Some(&value) {
                    edits.push(Edit::Set {
                        doc: document.id.clone(),
                        id: id.clone(),
                        path: FieldPath::field(key),
                        value: Some(value),
                    });
                }
            }
        }
    }
    edits
}

fn references_value(
    value: &Value,
    source: &str,
    ids: &BTreeSet<String>,
    owner: Owner,
    path: &mut Vec<String>,
) -> bool {
    match value {
        Value::String(text) => {
            if owner.bare_path(path) && selected(text, source) {
                return true;
            }
            if let Ok(parts) = parse_ref(text)
                && (parts.path.is_some_and(|path| selected(path, source))
                    || parts.id.is_some_and(|id| ids.contains(id)))
            {
                return true;
            }
            text.match_indices('@').any(|(start, _)| {
                let candidate = &text[start..];
                ids.iter().any(|id| {
                    candidate
                        .match_indices(&format!("#{id}"))
                        .any(|(offset, _)| {
                            candidate[offset + id.len() + 1..]
                                .chars()
                                .next()
                                .is_none_or(|character| {
                                    !character.is_ascii_lowercase()
                                        && !character.is_ascii_digit()
                                        && character != '_'
                                })
                        })
                }) || candidate.strip_prefix('@').is_some_and(|path| {
                    inside(
                        path.split(['"', '\'', '<', '>', '\n'])
                            .next()
                            .unwrap_or(path)
                            .split('#')
                            .next()
                            .unwrap_or(path),
                        source,
                    )
                })
            })
        }
        Value::Array(values) => values.iter().enumerate().any(|(index, value)| {
            path.push(index.to_string());
            let found = references_value(value, source, ids, owner, path);
            path.pop();
            found
        }),
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            if references_value(
                &Value::String(key.clone()),
                source,
                ids,
                Owner::Generic,
                &mut Vec::new(),
            ) {
                return true;
            }
            path.push(key.clone());
            let found = references_value(value, source, ids, owner, path);
            path.pop();
            found
        }),
        _ => false,
    }
}

/// Read-only counterpart to `rewrite_json_paths`, used by safe-delete preflight
/// for canonical JSON owners outside the authored asset document index.
pub fn json_uses_path(
    owner: JsonOwner,
    source: &str,
    asset_ids: &BTreeSet<String>,
    value: &Value,
) -> bool {
    references_value(
        value,
        source,
        asset_ids,
        Owner::Json(owner),
        &mut Vec::new(),
    )
}

pub(super) fn blockers(workspace: &Workspace, source: &str, ids: &BTreeSet<String>) -> Vec<String> {
    let mut blockers = BTreeSet::new();
    for (path, document) in workspace.docs() {
        if ids.contains(document.id.as_str()) {
            continue;
        }
        for (record, fields) in document.records() {
            for (field, value) in fields {
                if references_value(
                    value,
                    source,
                    ids,
                    Owner::document(document),
                    &mut vec![field.clone()],
                ) {
                    blockers.insert(format!("{path} ({record}.{field})"));
                }
            }
        }
    }
    blockers.into_iter().collect()
}

#[cfg(test)]
mod tests;
