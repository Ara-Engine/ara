use crate::{Result, codes, error};
use ara_diag::Diagnostic;
use ara_doc::{AssetId, Document, Edit, RecordId};
use ara_schema::{AssetRef, DocKindDesc, FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Parameter types shared by messages, generated bindings and translation jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "L10nParamKind", module = "l10n")]
#[serde(rename_all = "camelCase")]
pub enum ParamKind {
    /// User or application text, inserted with bidi isolation.
    Text,
    /// Exact decimal, also usable as a plural selector.
    Number,
    /// ISO calendar date, formatted in the requested locale's calendar.
    Date,
    /// Exact decimal and ISO 4217 currency code.
    Currency,
}

/// Human review state, independent of whether the text can be displayed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "L10nStatus", module = "l10n")]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// A human has approved this translation.
    Reviewed,
    /// A translation provider generated this text.
    MachineTranslated,
    /// Text requires a translator's attention.
    #[default]
    NeedsReview,
}

/// Reproducible origin of a generated translation; no secrets belong here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "L10nProvenance", module = "l10n")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    /// Provider identifier chosen by the host.
    pub provider: String,
    /// Provider model/version identifier.
    pub model: String,
    /// Host-assigned generation job identifier.
    pub job: String,
    /// BLAKE3 of source text and typed parameter signature.
    pub source_hash: String,
}

/// Explicit font resource; never depends on installed OS fonts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "L10nFontFace", module = "l10n")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FontFace {
    /// Family name passed to `ara_ui_text::TextSystem` in fallback order.
    pub family: String,
    /// Font asset, using the standard `@path#id` reference syntax.
    pub asset: Option<AssetRef>,
}

/// One locale's configuration, stored as `l_config` in its localization document.
#[derive(Debug, Clone, Default, Schema, Serialize, Deserialize)]
#[ara(name = "L10nLocaleConfig", record(prefix = "l"), module = "l10n")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LocaleConfig {
    /// Canonical BCP-47 locale identifier such as `ar-EG`.
    pub locale: String,
    /// Additional fallback locales, followed after the locale's structural parents.
    pub fallback: Vec<String>,
    /// Mirror layout boxes; glyph shaping and logical reading order remain intact.
    pub rtl: bool,
    /// Ordered family/resources for this locale, inherited through fallback.
    pub fonts: Vec<FontFace>,
}

/// A mergeable message record; its stable record ID survives translation changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "L10nStringEntry", record(prefix = "m"), module = "l10n")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct StringEntry {
    /// Stable dotted lookup key, for example `inventory.items`.
    pub key: String,
    /// ICU MessageFormat pattern, including nested plural/select expressions.
    pub text: String,
    /// Exact parameter names and types; every locale must share this signature.
    pub params: BTreeMap<String, ParamKind>,
    /// Review state visible to agents and translators.
    pub status: Status,
    /// Generation origin, or none for human-authored source text.
    pub provenance: Option<Provenance>,
    /// Translator context, never displayed in game.
    pub note: String,
}

impl StringEntry {
    /// Source identity used to reject stale generation responses.
    pub fn source_hash(&self) -> String {
        let value = serde_json::json!({"text": self.text, "params": self.params});
        blake3::hash(ara_json::to_canonical_string(&ara_json::sort_keys(&value)).as_bytes())
            .to_hex()
            .to_string()
    }
}

/// Registers localization records and the `.l10n` canonical document kind.
pub fn register_schema(db: &mut SchemaDb) {
    db.register::<LocaleConfig>().register::<StringEntry>();
    if db.kind("l10n").is_none() {
        let _ = db.add_kind(DocKindDesc {
            name: "l10n".into(),
            version: 1,
            doc: "One locale's messages, review provenance and explicit font fallback.".into(),
            id_prefix: "lc".into(),
            extension: "l10n".into(),
            records: vec!["L10nLocaleConfig".into(), "L10nStringEntry".into()],
            single_root: false,
        });
    }
}

/// Read-only validated view of one canonical localization document.
#[derive(Debug, Clone)]
pub struct Table {
    document: Document,
    pub(crate) config: LocaleConfig,
    pub(crate) entries: BTreeMap<String, (RecordId, StringEntry)>,
}

impl Table {
    /// Creates an empty authoring document; initialize it through a typed command.
    pub fn empty(id: AssetId) -> Document {
        Document::new("l10n", id)
    }

    /// Reads using the canonical kernel reader and validates message signatures.
    pub fn read(text: &str, path: &str, db: &SchemaDb) -> Result<Self> {
        let read = ara_doc::read(text, path, db, None);
        if let Some(d) = read.diags.into_iter().find(Diagnostic::is_error) {
            return Err(d);
        }
        Self::from_document(
            read.doc
                .ok_or_else(|| error("missing localization document"))?,
        )
    }

    /// Compiles a store document; mutation still belongs to the store's transaction.
    pub fn from_document(document: Document) -> Result<Self> {
        if document.kind != "l10n" || !document.conflicts.is_empty() {
            return Err(error("expected a conflict-free l10n document"));
        }
        let record = document
            .record("l_config")
            .ok_or_else(|| error("missing l_config"))?;
        let mut config: LocaleConfig =
            serde_json::from_value(record.clone().into()).map_err(|e| error(e.to_string()))?;
        config.locale = normalize_locale(&config.locale)?;
        for fallback in &mut config.fallback {
            *fallback = normalize_locale(fallback)?;
        }
        let mut entries = BTreeMap::new();
        for (id, record) in document.records() {
            if id.as_str() == "l_config" {
                continue;
            }
            if id.prefix() != "m" {
                return Err(error("only l_config and message records are allowed"));
            }
            let entry: StringEntry =
                serde_json::from_value(record.clone().into()).map_err(|e| error(e.to_string()))?;
            validate_key(&entry.key)?;
            crate::Message::parse(&entry.text, &entry.params)?;
            if entries
                .insert(entry.key.clone(), (id.clone(), entry))
                .is_some()
            {
                return Err(error("duplicate localization key"));
            }
        }
        Ok(Self {
            document,
            config,
            entries,
        })
    }

    /// Locale metadata and ordered font fallback resources.
    pub fn config(&self) -> &LocaleConfig {
        &self.config
    }
    /// Messages ordered by stable lookup key.
    pub fn entries(&self) -> impl Iterator<Item = &StringEntry> {
        self.entries.values().map(|(_, entry)| entry)
    }
    /// Lookup a message without following locale fallback.
    pub fn entry(&self, key: &str) -> Option<&StringEntry> {
        self.entries.get(key).map(|(_, entry)| entry)
    }
    /// The underlying document, suitable for canonical writing or transaction inspection.
    pub fn document(&self) -> &Document {
        &self.document
    }
    /// Writes through Ara's canonical document writer.
    pub fn write(&self, db: &SchemaDb) -> String {
        ara_doc::write(&self.document, db)
    }
}

/// Typed localization mutations translated to the standard inverse-capable ops.
#[derive(Debug, Clone)]
pub enum TableCommand {
    /// Initializes or replaces locale metadata.
    Configure(LocaleConfig),
    /// Inserts/replaces a whole message at a stable record ID.
    Put {
        /// Stable `m_` record ID.
        id: RecordId,
        /// Validated message and parameter signature.
        entry: StringEntry,
    },
    /// Removes an obsolete message.
    Remove(RecordId),
}

impl TableCommand {
    /// Produces edits for `Store::transact`; never modifies documents directly.
    pub fn edits(self, doc: &Document) -> Result<Vec<Edit>> {
        let (id, value) = match self {
            Self::Configure(mut config) => {
                config.locale = normalize_locale(&config.locale)?;
                for fallback in &mut config.fallback {
                    *fallback = normalize_locale(fallback)?;
                }
                (
                    RecordId::parse("l_config").map_err(error)?,
                    serde_json::to_value(config).map_err(|e| error(e.to_string()))?,
                )
            }
            Self::Put { id, entry } => {
                if id.prefix() != "m" {
                    return Err(error("message IDs must start with m_"));
                }
                if doc.records().iter().any(|(existing_id, record)| {
                    existing_id != &id
                        && record.get("key").and_then(serde_json::Value::as_str) == Some(&entry.key)
                }) {
                    return Err(error("message key already belongs to another record"));
                }
                validate_key(&entry.key)?;
                crate::Message::parse(&entry.text, &entry.params)?;
                (
                    id,
                    serde_json::to_value(entry).map_err(|e| error(e.to_string()))?,
                )
            }
            Self::Remove(id) => {
                return Ok(vec![Edit::Delete {
                    doc: doc.id.clone(),
                    id,
                }]);
            }
        };
        let record = value.as_object().ok_or_else(|| error("expected record"))?;
        if let Some(old) = doc.record(id.as_str()) {
            let keys: std::collections::BTreeSet<_> = old.keys().chain(record.keys()).collect();
            Ok(keys
                .into_iter()
                .map(|key| Edit::Set {
                    doc: doc.id.clone(),
                    id: id.clone(),
                    path: FieldPath::field(key),
                    value: record.get(key).cloned(),
                })
                .collect())
        } else {
            Ok(vec![Edit::Create {
                doc: doc.id.clone(),
                id,
                record: record.clone(),
            }])
        }
    }
}

pub(crate) fn normalize_locale(locale: &str) -> Result<String> {
    locale
        .parse::<icu_locale::Locale>()
        .map(|l| l.to_string())
        .map_err(|e| Diagnostic::error(codes::TABLE, format!("invalid locale {locale}: {e}")))
}

pub(crate) fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.split('.').any(|s| {
            s.is_empty()
                || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                || !s.starts_with(|c: char| c.is_ascii_alphabetic())
        })
    {
        return Err(error(format!(
            "invalid localization key {key:?}; use dotted ASCII identifiers"
        )));
    }
    Ok(())
}
