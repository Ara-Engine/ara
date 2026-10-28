//! Transactional projection of lossless native `.ui` source into kernel records.
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Document, Edit, ReadResult, RecordId};
use ara_schema::{DocKindDesc, FieldPath, Schema, SchemaDb};
use ara_ui_document::{DocumentKind, Patch, SourceDocument, SourceOp};
use serde::Deserialize;
use serde_json::{Value, json};

const HEADER: &str = "<!-- ara:asset ";

/// Lossless UI source; persistence writes native XML through `SourceDocument`.
#[derive(Schema)]
#[ara(record(prefix = "us"), module = "ui")]
pub struct UiSource {
    /// Complete XML, including comments and stable asset identity metadata.
    pub source: String,
}

fn invalid(message: impl ToString) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message.to_string())
}

fn record_id() -> RecordId {
    RecordId::parse("us_source").expect("valid UI source record ID")
}

fn metadata(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(HEADER)?.strip_suffix(" -->"))
}

fn identity(text: &str, path: &str) -> Result<AssetId, Diagnostic> {
    if let Some(id) = metadata(text) {
        let id = AssetId::parse(id).map_err(invalid)?;
        if id.prefix() != "ui" {
            return Err(invalid("UI asset IDs start with ui_"));
        }
        return Ok(id);
    }
    // Legacy markup remains readable before its first command establishes metadata.
    let elements = ara_ui_document::parse(text).map_err(invalid)?;
    if let Some(id) = elements
        .first()
        .and_then(|e| AssetId::parse(&e.id).ok())
        .filter(|id| id.prefix() == "ui")
    {
        return Ok(id);
    }
    AssetId::parse(&format!("ui_{:x}", ara_ui_document::stable_id(path))).map_err(invalid)
}

/// Pins an existing identity before a source file changes its storage path.
pub(crate) fn with_identity(text: &str, id: &AssetId) -> Result<String, Diagnostic> {
    if let Some(found) = metadata(text) {
        if found != id.as_str() {
            return Err(invalid("UI asset identity cannot change"));
        }
        Ok(text.to_owned())
    } else {
        if text.starts_with("<?xml")
            && let Some(end) = text.find("?>")
        {
            let end = end + 2;
            return Ok(format!(
                "{}\n{HEADER}{id} -->\n{}",
                &text[..end],
                &text[end..]
            ));
        }
        Ok(format!("{HEADER}{id} -->\n{text}"))
    }
}

pub(crate) fn header(text: &str, path: &str) -> Option<(String, AssetId)> {
    SourceDocument::parse(text, DocumentKind::Markup).ok()?;
    Some(("ui".into(), identity(text, path).ok()?))
}

pub(crate) fn read(text: &str, path: &str) -> ReadResult {
    let parsed: Result<Document, Diagnostic> = (|| {
        let source = SourceDocument::parse(text, DocumentKind::Markup).map_err(invalid)?;
        let id = identity(source.serialize(), path)?;
        let mut doc = Document::new("ui", id);
        doc.insert_unchecked(
            record_id(),
            serde_json::Map::from_iter([("source".into(), source.serialize().into())]),
        );
        Ok(doc)
    })();
    match parsed {
        Ok(doc) => ReadResult {
            doc: Some(doc),
            diags: Vec::new(),
        },
        Err(diagnostic) => ReadResult {
            doc: None,
            diags: vec![diagnostic.file(path)],
        },
    }
}

fn source(doc: &Document) -> Result<&str, Diagnostic> {
    doc.record("us_source")
        .and_then(|r| r.get("source"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("UI document requires its us_source record"))
}

pub(crate) fn validate(doc: &Document, path: &str) -> Result<(), Diagnostic> {
    if doc.kind != "ui" {
        return Ok(());
    }
    let text = source(doc)?;
    SourceDocument::parse(text, DocumentKind::Markup).map_err(invalid)?;
    if identity(text, path)? != doc.id {
        return Err(invalid("UI source identity differs from its document"));
    }
    Ok(())
}

pub(crate) fn write(doc: &Document) -> Result<String, Diagnostic> {
    if !doc.conflicts.is_empty() {
        return Err(invalid(
            "UI source has unresolved conflicts; reconcile it with doc.sync before saving",
        ));
    }
    let source = SourceDocument::parse(source(doc)?, DocumentKind::Markup).map_err(invalid)?;
    Ok(source.serialize().to_owned())
}

fn create(cx: &mut EditCtx<'_, '_>, path: &str, id: Option<&str>) -> Result<AssetId, Diagnostic> {
    let id = cx.create_document(path, "ui", id)?;
    let text = with_identity(&format!("<ui id=\"{id}\" />\n"), &id)?;
    let source = SourceDocument::parse(&text, DocumentKind::Markup).map_err(invalid)?;
    cx.apply_edit(Edit::Create {
        doc: id.clone(),
        id: record_id(),
        record: serde_json::Map::from_iter([("source".into(), source.serialize().into())]),
    })?;
    Ok(id)
}

/// Replaces a UI document's native markup through revision-checked source ops.
#[derive(Schema, Deserialize)]
#[ara(params, module = "ui")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiSourceApplyParams {
    /// Complete native UI XML. Existing identity metadata may be retained or omitted.
    pub source: String,
}
struct Apply;
impl Command for Apply {
    type Params = UiSourceApplyParams;
    type Output = Value;
    const NAME: &'static str = "ui.source.apply";
    const SUMMARY: &'static str =
        "Apply lossless native UI markup with validation, attribution and undo.";
    fn examples() -> Vec<Value> {
        vec![json!({"source":"<ui id=\"hud\"><text id=\"title\" text=\"Hello\" /></ui>\n"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        let doc = cx.read().doc();
        if doc.kind != "ui" {
            return Err(invalid("expected UI document"));
        }
        let text = source(doc)?;
        let mut document = SourceDocument::parse(text, DocumentKind::Markup).map_err(invalid)?;
        let after = with_identity(&p.source, &doc.id)?;
        let op = SourceOp {
            revision: document.revision(),
            patches: vec![Patch {
                bytes: 0..text.len(),
                before: text.to_owned(),
                after,
            }],
        };
        document.apply(&op).map_err(invalid)?;
        cx.set(
            &record_id(),
            FieldPath::field("source"),
            Some(document.serialize().into()),
        )?;
        Ok(json!({"id":record_id()}))
    }
}

pub(crate) fn register(registry: &mut Registry, db: &mut SchemaDb) -> Vec<Diagnostic> {
    db.register::<UiSource>();
    let result = db.add_kind(DocKindDesc {
        name: "ui".into(),
        version: 1,
        id_prefix: "ui".into(),
        extension: "ui".into(),
        doc: "Lossless native UI XML, edited through attributed source operations.".into(),
        records: vec!["UiSource".into()],
        single_root: false,
    });
    registry
        .document_template("ui", create)
        .command::<Apply>(db);
    result.err().into_iter().collect()
}
