//! Editing a document file's JSON by record and field path, for the file-level
//! repairs `ara resolve` and `ara check --fix`. The result is written back in
//! canonical form (through the document reader and writer when it reads cleanly).

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::project::Project;

/// The keys of a field path: `Light.color` → `[Light, color]`,
/// `overrides[e_c001/Light.color]` → `[overrides, e_c001/Light.color]`.
pub fn segments(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        match c {
            '.' => out.push(std::mem::take(&mut cur)),
            '[' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let key: String = chars.by_ref().take_while(|&c| c != ']').collect();
                out.push(key);
                if chars.clone().next() == Some('.') {
                    chars.next();
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A document file as JSON.
pub struct DocFile {
    /// The parsed file.
    pub root: Map<String, Value>,
}

impl DocFile {
    /// Reads `path`.
    pub fn read(path: &Path) -> Result<DocFile, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
        match ara_json::parse(&text) {
            Ok(Value::Object(root)) => Ok(DocFile { root }),
            Ok(_) => Err(format!("`{}` is not a JSON object", path.display())),
            Err(d) => Err(d.to_string()),
        }
    }

    /// The object at `path` inside record `record` (`path` empty: the record), made
    /// if missing; with the last key split off. `None` if a step is not an object.
    fn parent(&mut self, record: &str, path: &str) -> Option<(&mut Map<String, Value>, String)> {
        let records = self.root.get_mut("records")?.as_object_mut()?;
        let mut keys = segments(path);
        let Some(last) = keys.pop() else {
            return Some((records, record.to_owned()));
        };
        let mut cur = records.get_mut(record)?.as_object_mut()?;
        for k in keys {
            cur = cur
                .entry(k)
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()?;
        }
        Some((cur, last))
    }

    /// Sets (`Some`) or removes (`None`) the value at `path` of `record`.
    pub fn set(&mut self, record: &str, path: &str, value: Option<Value>) -> bool {
        let Some((obj, key)) = self.parent(record, path) else {
            return false;
        };
        match value {
            Some(v) => {
                obj.insert(key, v);
            }
            None => {
                obj.shift_remove(&key);
            }
        }
        true
    }

    /// Renames the last key of `path` in `record` to `to`, keeping its value.
    pub fn rename(&mut self, record: &str, path: &str, to: &str) -> bool {
        let Some((obj, key)) = self.parent(record, path) else {
            return false;
        };
        match obj.shift_remove(&key) {
            Some(v) => {
                obj.insert(to.to_owned(), v);
                true
            }
            None => false,
        }
    }

    /// Writes the file canonically: through the document reader and writer when the
    /// result reads, else as canonical JSON.
    pub fn write(self, project: &Project, path: &Path) -> Result<(), String> {
        let json = ara_json::to_canonical_string(&Value::Object(self.root));
        let rel = project.rel(path);
        let read = ara_doc::read(&json, &rel, &project.db, Some(&project.index));
        let text = read
            .doc
            .map_or(json, |doc| ara_doc::write(&doc, &project.db));
        fs::write(path, text).map_err(|e| format!("cannot write `{rel}`: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::segments;

    #[test]
    fn field_paths_split_into_keys() {
        assert_eq!(segments("name"), ["name"]);
        assert_eq!(segments("Light.color"), ["Light", "color"]);
        assert_eq!(
            segments("overrides[e_c001/Light.color]"),
            ["overrides", "e_c001/Light.color"]
        );
        assert_eq!(segments("a[b.c].d"), ["a", "b.c", "d"]);
    }
}
