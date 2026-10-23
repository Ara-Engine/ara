use super::{Translated, put};
use crate::{Provenance, Result, Status, Table, codes, error};
use ara_cmd::{Job, JobCtx};
use ara_diag::Diagnostic;
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One provider input; the source fingerprint makes late responses safe to reject.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslationItem {
    /// Stable source key.
    pub key: String,
    /// Complete ICU MessageFormat source text.
    pub source: String,
    /// Source text plus parameter signature fingerprint.
    pub source_hash: String,
    /// Typed parameter signature the provider must preserve.
    pub params: BTreeMap<String, crate::ParamKind>,
    /// Human context for the translator/provider.
    pub note: String,
    /// Existing target fingerprint, or empty if absent; prevents late overwrites.
    pub target_hash: String,
}
/// Provider-independent generation job, suitable for a background worker or MCP host.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslationRequest {
    /// Stable host job ID used in provenance and response matching.
    pub job: String,
    /// Source table locale.
    pub source_locale: String,
    /// Destination table locale.
    pub target_locale: String,
    /// Whole messages to translate without changing keys or signatures.
    pub items: Vec<TranslationItem>,
}
/// Provider response; no provider SDK or credentials enter the localization crate.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslationResponse {
    /// Request's exact job ID.
    pub job: String,
    /// Provider identifier for provenance.
    pub provider: String,
    /// Model/version identifier for provenance.
    pub model: String,
    /// Complete translated ICU patterns keyed by the requested source keys.
    pub translations: BTreeMap<String, String>,
}
/// Host-owned provider interface. Run off the UI thread; commit with TranslateJob.
pub trait TranslationProvider {
    /// Generates translations with an arbitrary SDK, local model or human service.
    fn generate(&mut self, request: &TranslationRequest) -> Result<TranslationResponse>;
}
/// Selects missing/unreviewed messages for generation, preserving reviewed entries.
pub fn prepare_translation(
    source: &Table,
    target: &Table,
    job: impl Into<String>,
) -> TranslationRequest {
    TranslationRequest {
        job: job.into(),
        source_locale: source.config().locale.clone(),
        target_locale: target.config().locale.clone(),
        items: source
            .entries()
            .filter(|e| {
                target
                    .entry(&e.key)
                    .is_none_or(|t| t.status != Status::Reviewed)
            })
            .map(|entry| TranslationItem {
                key: entry.key.clone(),
                source: entry.text.clone(),
                source_hash: entry.source_hash(),
                params: entry.params.clone(),
                note: entry.note.clone(),
                target_hash: target
                    .entry(&entry.key)
                    .map_or(String::new(), crate::StringEntry::source_hash),
            })
            .collect(),
    }
}
/// Runs and validates a provider response without mutating any authoring document.
pub fn run_provider(
    provider: &mut impl TranslationProvider,
    request: &TranslationRequest,
) -> Result<TranslationResponse> {
    let response = provider.generate(request)?;
    validate_response(request, &response)?;
    Ok(response)
}
fn validate_response(request: &TranslationRequest, response: &TranslationResponse) -> Result<()> {
    if request.job.is_empty()
        || request.job != response.job
        || response.provider.is_empty()
        || response.model.is_empty()
        || request.items.len() != response.translations.len()
    {
        return Err(Diagnostic::error(
            codes::TRANSLATION,
            "translation response identity/key count mismatch",
        ));
    }
    let mut keys = std::collections::BTreeSet::new();
    for item in &request.items {
        if !keys.insert(&item.key) {
            return Err(error("duplicate translation request key"));
        }
        let text = response
            .translations
            .get(&item.key)
            .ok_or_else(|| error("translation response key mismatch"))?;
        crate::Message::parse(text, &item.params)?;
    }
    Ok(())
}
/// Generation request and completed provider output, committed as one undoable job.
#[derive(Schema, Deserialize)]
#[ara(params, module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslateParams {
    /// Original generation request, including source and target fingerprints.
    pub request: TranslationRequest,
    /// Completed response from the host's selected provider.
    pub response: TranslationResponse,
}
/// Atomic provenance-bearing application of a provider-agnostic translation job.
pub struct TranslateJob;
impl Job for TranslateJob {
    type Params = TranslateParams;
    type Output = Translated;
    const NAME: &'static str = "l10n.translate_job";
    const SUMMARY: &'static str =
        "Validate and apply a generation job with provenance and review state.";
    const SCOPE: &'static str = "edit:l10n";
    fn examples() -> Vec<Value> {
        vec![
            json!({"request":{"job":"job1","sourceLocale":"en","targetLocale":"ar","items":[]},"response":{"job":"job1","provider":"local","model":"translator-v1","translations":{}}}),
        ]
    }
    fn run(ctx: &mut JobCtx<'_, '_>, params: Self::Params) -> Result<Self::Output> {
        let request = params.request;
        let response = params.response;
        validate_response(&request, &response)?;
        let target = Table::from_document(ctx.edit.read().doc().clone())?;
        if target.config().locale != request.target_locale {
            return Err(error("translation job targets a different locale"));
        }
        let source = ctx
            .edit
            .read()
            .workspace()
            .docs()
            .filter(|(_, d)| d.kind == "l10n")
            .map(|(_, d)| Table::from_document(d.clone()))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .find(|t| t.config().locale == request.source_locale)
            .ok_or_else(|| error("translation source table is not loaded"))?;
        let count = request.items.len();
        for (index, item) in request.items.into_iter().enumerate() {
            let original = source
                .entry(&item.key)
                .ok_or_else(|| error("translation source key removed"))?;
            if original.source_hash() != item.source_hash
                || original.note != item.note
                || original.text != item.source
                || original.params != item.params
                || target
                    .entry(&item.key)
                    .map_or(String::new(), crate::StringEntry::source_hash)
                    != item.target_hash
                || target
                    .entry(&item.key)
                    .is_some_and(|e| e.status == Status::Reviewed)
            {
                return Err(Diagnostic::error(
                    codes::TRANSLATION,
                    format!("stale translation for {}", item.key),
                ));
            }
            let mut entry = original.clone();
            entry.text = response.translations[&item.key].clone();
            entry.status = Status::MachineTranslated;
            entry.provenance = Some(Provenance {
                provider: response.provider.clone(),
                model: response.model.clone(),
                job: response.job.clone(),
                source_hash: item.source_hash,
            });
            put(&mut ctx.edit, None, entry)?;
            ctx.progress(
                (index + 1) as f32 / count as f32,
                "Translated message staged",
            );
        }
        Ok(Translated { count })
    }
}
