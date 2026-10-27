//! Adapt the legacy project marker to an internal document for transactions and undo.
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, AssetIndex, Document, ReadResult, RecordId};
use ara_schema::SchemaDb;
use serde_json::Value;

pub(crate) fn check_creations(
    root: &std::path::Path,
    ops: &[ara_doc::Op],
) -> Result<(), Vec<Diagnostic>> {
    let planned: std::collections::BTreeMap<_, _> = ops
        .iter()
        .filter_map(|op| {
            if let ara_doc::Op::File { path, state, .. } = op {
                Some((path.as_str(), state.as_ref()))
            } else {
                None
            }
        })
        .collect();
    for op in ops {
        if let ara_doc::Op::Document {
            path,
            kind: Some(kind),
            ..
        } = op
        {
            if planned.contains_key(path.as_str()) {
                check_parent(root, path).map_err(|d| vec![d])?;
            } else {
                check_new_path(root, path).map_err(|d| vec![d])?;
            }
            if kind == "import" {
                let source = path
                    .strip_suffix(".import.json")
                    .expect("import documents have a sidecar extension");
                if let Some(state) = planned.get(source) {
                    if state.is_some_and(|s| s.kind == ara_doc::file_state::FileKind::File) {
                        continue;
                    }
                    return Err(vec![Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "source media is removed by this batch",
                    )]);
                }
                let source = root.join(source).canonicalize().map_err(|e| {
                    vec![Diagnostic::error(
                        codes::IO,
                        format!("source media is missing: {e}"),
                    )]
                })?;
                if !source.starts_with(root) || !source.is_file() {
                    return Err(vec![Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "source must be a file inside the project",
                    )]);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn check_new_path(root: &std::path::Path, path: &str) -> Result<(), Diagnostic> {
    let target = root.join(path);
    if target.symlink_metadata().is_ok() {
        return Err(Diagnostic::error(
            codes::INVALID_PARAMS,
            format!("file `{path}` already exists"),
        ));
    }
    check_parent(root, path)
}

pub(crate) fn check_parent(root: &std::path::Path, path: &str) -> Result<(), Diagnostic> {
    let target = root.join(path);
    let ancestor = target
        .parent()
        .and_then(|p| p.ancestors().find(|p| p.exists()))
        .ok_or_else(|| {
            Diagnostic::error(codes::INVALID_PARAMS, "document has no existing parent")
        })?;
    let resolved = ancestor
        .canonicalize()
        .map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?;
    if !resolved.starts_with(root) {
        return Err(Diagnostic::error(
            codes::INVALID_PARAMS,
            "document parent escapes the project",
        ));
    }
    Ok(())
}

impl crate::host::State {
    pub(crate) fn refresh_project_settings(&mut self) {
        if let Some(settings) = self
            .store
            .workspace()
            .doc("pj_project")
            .and_then(|d| d.record("p_settings"))
            .and_then(|r| r.get("settings"))
        {
            self.main = settings
                .get("main")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(name) = settings.get("name").and_then(Value::as_str) {
                self.name = name.to_owned();
            }
        }
    }
}

pub(crate) fn read(
    text: &str,
    file: &str,
    db: &SchemaDb,
    index: Option<&AssetIndex>,
) -> ReadResult {
    if (file.ends_with(".ui") || text.trim_start().starts_with("<!-- ara:asset ui_"))
        && !text.trim_start().starts_with('{')
    {
        return crate::ui_document::read(text, file);
    }
    if file != crate::project::MARKER {
        return ara_doc::read(text, file, db, index);
    }
    let value = match ara_json::parse(text) {
        Ok(Value::Object(value)) => Value::Object(value),
        other => {
            return ReadResult {
                doc: None,
                diags: vec![
                    Diagnostic::error(
                        codes::INVALID_PARAMS,
                        format!("invalid project settings object: {other:?}"),
                    )
                    .file(file),
                ],
            };
        }
    };
    let mut doc = Document::new(
        "project",
        AssetId::parse("pj_project").expect("valid project ID"),
    );
    doc.insert_unchecked(
        RecordId::parse("p_settings").expect("valid settings ID"),
        serde_json::Map::from_iter([("settings".into(), value)]),
    );
    ReadResult {
        doc: Some(doc),
        diags: Vec::new(),
    }
}

pub(crate) fn read_header(text: &str, file: &str, db: &SchemaDb) -> Option<(String, AssetId)> {
    if file.ends_with(".ui") && !text.trim_start().starts_with('{') {
        crate::ui_document::header(text, file)
    } else {
        ara_doc::read_header(text, db)
    }
}

pub(crate) fn write(doc: &Document, db: &SchemaDb) -> Result<String, Diagnostic> {
    if doc.kind == "project" {
        let settings = doc
            .record("p_settings")
            .and_then(|r| r.get("settings"))
            .expect("project document retains its settings record");
        Ok(ara_json::to_canonical_string(&ara_json::sort_keys(
            settings,
        )))
    } else if doc.kind == "ui" {
        crate::ui_document::write(doc)
    } else {
        Ok(ara_doc::write(doc, db))
    }
}
