//! Claim subjects from text: `e_door01`, `e_4f9a/e_m4tz`, `name:Gate Door`,
//! `e_door01.Light`, `levels/outpost.scene`, `@scripts/Gate.cs`.

use ara_collab::codes;
use ara_collab::{Subject, Wanted};
use ara_diag::Diagnostic;
use ara_doc::{AssetId, RecordId};
use ara_schema::FieldPath;
use serde_json::Value;

use crate::host::State;

fn invalid(msg: String) -> Diagnostic {
    Diagnostic::error(codes::INVALID_SUBJECT, msg)
}

/// Parses one subject; `doc` names the document entity subjects are in (default:
/// the one loaded document holding the entity).
pub(crate) fn parse(st: &State, text: &str, doc: Option<&str>) -> Result<Wanted, Diagnostic> {
    let ws = st.store.workspace();
    let text = text.trim();
    if let Some(id) = ws.find(text) {
        let path = ws.path_of(id.as_str()).unwrap_or(id.as_str()).to_owned();
        return Ok(Wanted {
            subject: Subject::Doc { doc: id.clone() },
            label: path,
        });
    }
    if text.starts_with('@') && !text.contains('#') {
        return Ok(Wanted {
            subject: Subject::Asset {
                path: text.to_owned(),
            },
            label: text.to_owned(),
        });
    }
    // `@levels/a.scene#e_1` names the document and the entity together.
    let (doc, rest) = match text.strip_prefix('@').and_then(|t| t.split_once('#')) {
        Some((path, rest)) => (Some(path), rest),
        None => (doc, text),
    };
    let (address, path) = match rest.strip_prefix("name:") {
        Some(name) => (by_name(st, name.trim(), doc)?, None),
        None => match rest.split_once('.') {
            Some((a, p)) => (a.to_owned(), Some(p)),
            None => (rest.to_owned(), None),
        },
    };
    let record = address.split('/').next().unwrap_or(&address);
    let record = RecordId::parse(record).map_err(|_| {
        invalid(format!(
            "`{text}` is not a document, entity, `name:` selector or `@path`"
        ))
    })?;
    let doc = match doc {
        Some(d) => ws
            .find(d)
            .cloned()
            .ok_or_else(|| invalid(format!("no document `{d}` is loaded")))?,
        None => holder(st, &record)?,
    };
    if ws
        .doc(doc.as_str())
        .and_then(|d| d.record(record.as_str()))
        .is_none()
    {
        return Err(invalid(format!("`{doc}` has no entity `{record}`")));
    }
    let doc_path = ws.path_of(doc.as_str()).unwrap_or(doc.as_str()).to_owned();
    let name = ws
        .doc(doc.as_str())
        .and_then(|d| d.record(record.as_str()))
        .and_then(|r| r.get("name"))
        .and_then(Value::as_str)
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();
    let subject = match path {
        Some(p) => Subject::Field {
            doc: doc.clone(),
            entity: record.clone(),
            path: p
                .parse::<FieldPath>()
                .map_err(|e| invalid(format!("`{text}`: {e}")))?,
        },
        None => Subject::Entity {
            doc: doc.clone(),
            entity: record.clone(),
        },
    };
    let tail = path.map(|p| format!(".{p}")).unwrap_or_default();
    Ok(Wanted {
        subject,
        label: format!("{doc_path}#{address}{tail}{name}"),
    })
}

/// The one loaded document holding `record`.
fn holder(st: &State, record: &RecordId) -> Result<AssetId, Diagnostic> {
    let ws = st.store.workspace();
    let found: Vec<&AssetId> = ws
        .docs()
        .filter(|(_, d)| d.record(record.as_str()).is_some())
        .map(|(_, d)| &d.id)
        .collect();
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(invalid(format!(
            "no loaded document has an entity `{record}`"
        ))),
        _ => Err(invalid(format!(
            "`{record}` is in several documents; pass `doc`"
        ))),
    }
}

/// The address of the one entity named `name` (in `doc`, or anywhere).
fn by_name(st: &State, name: &str, doc: Option<&str>) -> Result<String, Diagnostic> {
    let ws = st.store.workspace();
    let only = doc.and_then(|d| ws.find(d)).cloned();
    let mut found = Vec::new();
    for (_, d) in ws.docs() {
        if only.as_ref().is_some_and(|o| *o != d.id) {
            continue;
        }
        for (id, r) in d.records() {
            if r.get("name").and_then(Value::as_str) == Some(name) {
                found.push(id.to_string());
            }
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(invalid(format!("no entity named `{name}`"))),
        _ => Err(invalid(format!(
            "{} entities are named `{name}`; use an ID",
            found.len()
        ))),
    }
}
