//! Registry commands and provider-agnostic generation jobs. Source scanners return
//! replacement spans; hosts apply code edits separately from table transactions.
mod extract;
mod translate;
pub use extract::{Extract, ExtractParams, Extraction, SourceKind, extract};
pub use translate::{
    TranslateJob, TranslateParams, TranslationItem, TranslationProvider, TranslationRequest,
    TranslationResponse, prepare_translation, run_provider,
};

use crate::{Catalog, Result, StringEntry, Table, TableCommand, error};
use ara_cmd::{Command, EditCtx, Query, ReadCtx, Registry};
use ara_diag::Diagnostic;
use ara_doc::RecordId;
use ara_schema::{Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Registers localization queries, extraction, authoring fixes and translation jobs.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    crate::register_schema(db);
    registry.query::<FindMissing>(db);
    registry.command::<Put>(db);
    registry.command::<Remove>(db);
    registry.command::<Extract>(db);
    registry.job::<TranslateJob>(db);
}

/// Source locale for parity checks against the current batch document.
#[derive(Schema, Deserialize)]
#[ara(name = "L10nFindMissingParams", params, module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FindMissingParams {
    /// Source locale loaded in the document workspace, normally `en`.
    pub source_locale: String,
}
/// Finds missing/extra keys with structured ARA7401/ARA7402 diagnostics and fixes.
pub struct FindMissing;
impl Query for FindMissing {
    type Params = FindMissingParams;
    type Output = Vec<Diagnostic>;
    const NAME: &'static str = "l10n.find_missing";
    const SUMMARY: &'static str = "Find missing and extra locale keys with runnable fixes.";
    fn examples() -> Vec<Value> {
        vec![json!({"sourceLocale":"en"})]
    }
    fn run(ctx: &ReadCtx<'_>, params: Self::Params) -> Result<Self::Output> {
        let tables = ctx
            .workspace()
            .docs()
            .filter(|(_, d)| d.kind == "l10n")
            .map(|(_, d)| Table::from_document(d.clone()))
            .collect::<Result<Vec<_>>>()?;
        let target = Table::from_document(ctx.doc().clone())?;
        Catalog::new(&params.source_locale, tables)?.find_missing(&target.config().locale)
    }
}

/// Validated insertion/replacement params used by machine-applicable fixes.
#[derive(Schema, Deserialize)]
#[ara(name = "L10nPutParams", params, module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutParams {
    /// Existing record ID, or omit to reuse the key's record or a stable hashed ID.
    pub id: Option<String>,
    /// Whole message with typed parameters and review metadata.
    pub entry: StringEntry,
}
/// Inserts or replaces a message through the kernel transaction.
pub struct Put;
impl Command for Put {
    type Params = PutParams;
    type Output = String;
    const NAME: &'static str = "l10n.put";
    const SUMMARY: &'static str = "Insert or replace a typed localization message.";
    const SCOPE: &'static str = "edit:l10n";
    fn examples() -> Vec<Value> {
        vec![json!({"entry":{"key":"menu.resume","text":"Resume"}})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Self::Output> {
        put(ctx, params.id.as_deref(), params.entry)
    }
}

pub(crate) fn put(
    ctx: &mut EditCtx<'_, '_>,
    requested: Option<&str>,
    entry: StringEntry,
) -> Result<String> {
    let table = Table::from_document(ctx.read().doc().clone())?;
    let id = match requested {
        Some(id) => RecordId::parse(id).map_err(error)?,
        None => table
            .entries
            .get(&entry.key)
            .map(|(id, _)| Ok(id.clone()))
            .unwrap_or_else(|| record_id(&entry.key))?,
    };
    if table
        .entries
        .iter()
        .any(|(key, (existing, _))| existing == &id && key != &entry.key)
        || table
            .entries
            .get(&entry.key)
            .is_some_and(|(existing, _)| existing != &id)
    {
        return Err(error("message ID/key collision"));
    }
    let edits = TableCommand::Put {
        id: id.clone(),
        entry,
    }
    .edits(ctx.read().doc())?;
    for edit in edits {
        match edit {
            ara_doc::Edit::Create { id, record, .. } => ctx.create(id, record)?,
            ara_doc::Edit::Set {
                id, path, value, ..
            } => ctx.set(&id, path, value)?,
            _ => return Err(error("unexpected localization edit")),
        }
    }
    Ok(id.to_string())
}

/// Record addressed removal of an obsolete localization message.
#[derive(Schema, Deserialize)]
#[ara(name = "L10nRemoveParams", params, module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveParams {
    /// Stable message record ID, starting with `m_`.
    pub id: String,
}
/// Removes a message through the kernel transaction.
pub struct Remove;
impl Command for Remove {
    type Params = RemoveParams;
    type Output = bool;
    const NAME: &'static str = "l10n.remove";
    const SUMMARY: &'static str = "Remove an obsolete localization message.";
    const SCOPE: &'static str = "edit:l10n";
    fn examples() -> Vec<Value> {
        vec![json!({"id":"m_1234"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<bool> {
        let id = RecordId::parse(&params.id).map_err(error)?;
        if id.prefix() != "m" {
            return Err(error("only message records can be removed"));
        }
        ctx.delete(&id)?;
        Ok(true)
    }
}

pub(crate) fn record_id(key: &str) -> Result<RecordId> {
    RecordId::parse(&format!(
        "m_{}",
        &blake3::hash(key.as_bytes()).to_hex()[..20]
    ))
    .map_err(error)
}

/// Serializable count returned when a translation job commits successfully.
#[derive(Debug, Serialize)]
pub struct Translated {
    /// Number of records changed, all marked machine-translated.
    pub count: usize,
}
