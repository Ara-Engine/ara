use crate::{Arguments, FontFace, Message, Result, Table, codes, error, table::normalize_locale};
use ara_diag::{Diagnostic, FixAction};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct CompiledTable {
    table: Table,
    messages: BTreeMap<String, Message>,
}

/// Immutable runtime tables. Rebuild on background workers, then swap at a boundary.
#[derive(Clone)]
pub struct Catalog {
    source: String,
    tables: BTreeMap<String, CompiledTable>,
}

/// Story preview mode; uses the same pseudo expansion and mirror path as ara-ui-test.
#[derive(Debug, Clone, Copy, Default)]
pub enum Preview {
    /// Production text and the locale's configured direction.
    #[default]
    Native,
    /// Accent/expand text using the existing story pseudo-localizer.
    Pseudo,
    /// Accent/expand text and force RTL layout mirroring.
    RtlMirror,
}

impl Catalog {
    /// Formats a generated key with the parameter structure required by that key.
    pub fn text<P: crate::Parameters>(
        &self,
        locale: &str,
        key: crate::Key<P>,
        params: P,
    ) -> Result<String> {
        self.format(locale, key.id(), &params.into_arguments())
    }
    /// Compiles all tables, requiring a source table and identical parameter signatures.
    /// Missing/extra keys remain diagnosable and can still use fallback at runtime.
    pub fn new(source: &str, tables: impl IntoIterator<Item = Table>) -> Result<Self> {
        let source = normalize_locale(source)?;
        let mut compiled = BTreeMap::new();
        for table in tables {
            let locale = table.config.locale.clone();
            let messages = table
                .entries()
                .map(|e| Ok((e.key.clone(), Message::parse(&e.text, &e.params)?)))
                .collect::<Result<_>>()?;
            if compiled
                .insert(locale, CompiledTable { table, messages })
                .is_some()
            {
                return Err(error("duplicate locale table"));
            }
        }
        let catalog = Self {
            source,
            tables: compiled,
        };
        let base = catalog
            .tables
            .get(&catalog.source)
            .ok_or_else(|| error("source locale table is required"))?;
        for table in catalog.tables.values() {
            for entry in table.table.entries() {
                if base
                    .table
                    .entry(&entry.key)
                    .is_some_and(|base| base.params != entry.params)
                {
                    return Err(error(format!(
                        "parameter signature differs for {} in {}",
                        entry.key, table.table.config.locale
                    )));
                }
            }
        }
        Ok(catalog)
    }

    /// Locale → structural parents → configured fallbacks → source, deduplicated.
    /// Cycles in explicit fallback configuration terminate deterministically.
    pub fn fallback_chain(&self, locale: &str) -> Result<Vec<String>> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        self.visit_fallback(&normalize_locale(locale)?, &mut seen, &mut chain);
        self.visit_fallback(&self.source, &mut seen, &mut chain);
        Ok(chain)
    }

    fn visit_fallback(&self, locale: &str, seen: &mut BTreeSet<String>, chain: &mut Vec<String>) {
        if !seen.insert(locale.into()) {
            return;
        }
        chain.push(locale.into());
        if let Ok(mut parsed) = locale.parse::<icu_locale::Locale>() {
            parsed.extensions = Default::default();
            if parsed.to_string() != locale {
                self.visit_fallback(&parsed.to_string(), seen, chain);
            } else if !parsed.id.variants.is_empty() {
                parsed.id.variants.clear();
                self.visit_fallback(&parsed.to_string(), seen, chain);
            } else if parsed.id.region.take().is_some() || parsed.id.script.take().is_some() {
                self.visit_fallback(&parsed.to_string(), seen, chain);
            }
        }
        if let Some(table) = self.tables.get(locale) {
            for fallback in &table.table.config.fallback {
                self.visit_fallback(fallback, seen, chain);
            }
        }
    }

    /// Looks up a key with fallback and formats typed parameters with bidi isolation.
    pub fn format(&self, locale: &str, key: &str, args: &Arguments) -> Result<String> {
        for language in self.fallback_chain(locale)? {
            if let Some(message) = self.tables.get(&language).and_then(|t| t.messages.get(key)) {
                return message.format(&language, locale, args);
            }
        }
        Err(Diagnostic::error(
            codes::MISSING_KEY,
            format!("no translation or fallback for {key}"),
        ))
    }

    /// Source-key parity diagnostics with record-addressed, machine-applicable fixes.
    pub fn find_missing(&self, locale: &str) -> Result<Vec<Diagnostic>> {
        let locale = normalize_locale(locale)?;
        let target = self
            .tables
            .get(&locale)
            .ok_or_else(|| error("target table is not loaded"))?;
        let source = &self.tables[&self.source].table;
        let mut diagnostics = Vec::new();
        for entry in source.entries() {
            if target.table.entry(&entry.key).is_none() {
                let mut replacement = entry.clone();
                replacement.status = crate::Status::NeedsReview;
                replacement.provenance = None;
                let value = serde_json::to_value(replacement).map_err(|e| error(e.to_string()))?;
                diagnostics.push(
                    Diagnostic::warning(
                        codes::MISSING_KEY,
                        format!("{locale} is missing {}", entry.key),
                    )
                    .doc(target.table.document().id.to_string())
                    .with_fix(
                        "Insert source text and mark it for review",
                        FixAction::Command {
                            command: "l10n.put".into(),
                            params: serde_json::json!({"entry": value}),
                        },
                    ),
                );
            }
        }
        for (key, (id, _)) in &target.table.entries {
            if source.entry(key).is_none() {
                diagnostics.push(
                    Diagnostic::warning(codes::EXTRA_KEY, format!("{locale} has extra key {key}"))
                        .doc(target.table.document().id.to_string())
                        .record(id.to_string())
                        .with_fix(
                            "Remove obsolete key",
                            FixAction::Command {
                                command: "l10n.remove".into(),
                                params: serde_json::json!({"id":id}),
                            },
                        ),
                );
            }
        }
        Ok(diagnostics)
    }

    /// Ordered explicit font families/assets, inherited across the full fallback chain.
    pub fn fonts(&self, locale: &str) -> Result<Vec<&FontFace>> {
        let mut seen = BTreeSet::new();
        let mut fonts = Vec::new();
        for locale in self.fallback_chain(locale)? {
            if let Some(table) = self.tables.get(&locale) {
                for font in &table.table.config.fonts {
                    if seen.insert(&font.family) {
                        fonts.push(font);
                    }
                }
            }
        }
        Ok(fonts)
    }

    /// Builds a production story locale consumed directly by `ara_ui_test::Harness`.
    /// Parameterized keys require explicit fixture values; no placeholder guesses.
    pub fn story_locale(
        &self,
        locale: &str,
        fixtures: &BTreeMap<String, Arguments>,
        preview: Preview,
    ) -> Result<ara_ui_stories::Locale> {
        let chain = self.fallback_chain(locale)?;
        let rtl = matches!(preview, Preview::RtlMirror)
            || chain
                .iter()
                .find_map(|id| self.tables.get(id).map(|t| t.table.config.rtl))
                .unwrap_or(false);
        let mut messages = BTreeMap::new();
        for key in self.tables[&self.source].messages.keys() {
            messages.insert(
                key.clone(),
                self.format(locale, key, fixtures.get(key).unwrap_or(&Arguments::new()))?,
            );
        }
        Ok(ara_ui_stories::Locale {
            id: locale.into(),
            rtl,
            pseudo: !matches!(preview, Preview::Native),
            messages,
        })
    }

    /// Loaded locale tables for offline tools and code generation.
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values().map(|t| &t.table)
    }
}

/// Last-good catalog plus a prepared generation; I/O never occurs on publication.
pub struct LiveCatalog {
    current: Catalog,
    pending: Option<(u64, Catalog)>,
    generation: u64,
}

impl LiveCatalog {
    /// Recompiles one changed table against the latest staged tables. Feed this from
    /// committed ops or an ara-asset watcher on a background worker; invalid edits
    /// retain the last-good active and pending catalogs.
    pub fn reload_table(&mut self, generation: u64, table: Result<Table>) -> Result<bool> {
        if generation <= self.generation
            || self.pending.as_ref().is_some_and(|(g, _)| generation <= *g)
        {
            return Ok(false);
        }
        let table = table?;
        let base = self
            .pending
            .as_ref()
            .map_or(&self.current, |(_, catalog)| catalog);
        let locale = table.config.locale.clone();
        let mut tables: Vec<_> = base
            .tables()
            .filter(|t| t.config.locale != locale)
            .cloned()
            .collect();
        tables.push(table);
        let candidate = Catalog::new(&base.source, tables);
        self.stage(generation, candidate)
    }
    /// Initializes generation zero from already-compiled tables.
    pub fn new(current: Catalog) -> Self {
        Self {
            current,
            pending: None,
            generation: 0,
        }
    }
    /// Read-only active tables used by UI bindings.
    pub fn current(&self) -> &Catalog {
        &self.current
    }
    /// Active revision used to invalidate localized text caches.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Stages a fully compiled catalog. Older completions cannot replace newer ones.
    pub fn stage(&mut self, generation: u64, result: Result<Catalog>) -> Result<bool> {
        if generation <= self.generation
            || self.pending.as_ref().is_some_and(|(g, _)| generation <= *g)
        {
            return Ok(false);
        }
        let catalog = result?;
        self.pending = Some((generation, catalog));
        Ok(true)
    }
    /// Publishes only when the host's D20 apply policy allows this frame boundary.
    pub fn apply_boundary(&mut self, allowed: bool) -> bool {
        if !allowed {
            return false;
        }
        if let Some((generation, catalog)) = self.pending.take() {
            self.current = catalog;
            self.generation = generation;
            true
        } else {
            false
        }
    }
}
