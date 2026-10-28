//! Bounded, cursor-driven Problems projection. Console remains the underlying history.
use super::*;
mod metadata;
mod model;
use metadata::{Key, Metadata, Order, Phase, compile_kind, known_compile_kind};
use model::Scope;
#[cfg(test)]
mod tests;

const PAGE: usize = 500;
const MAX_ROWS: usize = 4096;
const MAX_SCOPES: usize = 128;
const MAX_RECORDS: usize = 8192;
type RowKey = (Key, Order, u64);

/// Current scoped compiler problems, separate from retained Console records.
#[derive(Default)]
pub(crate) struct CompilationView {
    cursor: u64,
    scopes: BTreeMap<Key, Scope>,
    rows: BTreeMap<RowKey, Entry>,
    protected: BTreeMap<u64, Entry>,
    records: BTreeMap<u64, (u64, u64)>,
    possible_gaps: u64,
    unknown_history: bool,
    invalid_metadata: u64,
    omitted_scopes: u64,
    omitted_rows: u64,
    omitted_protected: u64,
    read_error: Option<String>,
    version: u64,
    polled_revision: Option<u64>,
    #[cfg(test)]
    reads: u64,
}

impl Shell {
    /// Consume all severities before presentation filters; one bounded in-process query.
    pub(crate) fn poll_compile_problems(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let revision = editor.panels_revision;
        if self.compile_problems.polled_revision == Some(revision) {
            return Ok(false);
        }
        // Capture before the read: notifications arriving during logs/sync must
        // remain work for the next tick instead of being marked already polled.
        self.compile_problems.polled_revision = Some(revision);
        #[cfg(test)]
        {
            self.compile_problems.reads += 1;
        }
        let version = self.compile_problems.version;
        match editor.action("logs",json!({"group":false,"limit":PAGE,"since":self.compile_problems.cursor,"severity":"info"})) {
            Ok(page)=>self.compile_problems.ingest(&page),
            Err(error)=>self.compile_problems.read_failed(error.to_string()),
        }
        Ok(version != self.compile_problems.version)
    }

    /// Compact read-only acceptance evidence; no full log history or document copies.
    pub(crate) fn compile_problem_state(&self) -> Value {
        self.compile_problems.report()
    }
}

impl CompilationView {
    fn read_failed(&mut self, error: String) {
        if self.read_error.as_ref() != Some(&error) {
            self.version += 1;
        }
        self.read_error = Some(error);
    }

    fn ingest(&mut self, page: &Value) {
        let (Some(cursor), Some(total), Some(raw)) = (
            page["cursor"].as_u64(),
            page["total"].as_u64(),
            page["entries"].as_array(),
        ) else {
            self.read_failed(
                "Diagnostic cursor response was incomplete; previous compile problems are retained"
                    .into(),
            );
            return;
        };
        if cursor < self.cursor {
            self.read_failed(
                "Diagnostic cursor moved backwards; previous compile problems are retained".into(),
            );
            return;
        }
        if self.read_error.take().is_some() {
            self.version += 1;
        }
        let mut entries: Vec<_> = raw
            .iter()
            .filter_map(entry)
            .filter(|entry| entry.seq > self.cursor && entry.seq <= cursor)
            .collect();
        entries.sort_by_key(|entry| entry.seq);
        let mut accounted = 0u64;
        let mut uncertain = false;
        for entry in &entries {
            let delta = match self.records.get(&entry.first) {
                Some((count, sampled)) if entry.count >= *count && entry.sampled == *sampled => {
                    entry.count - *count
                }
                None if entry.first > self.cursor && entry.sampled == 0 => entry.count,
                _ => {
                    uncertain = true;
                    1
                }
            };
            accounted = accounted.saturating_add(delta);
            self.records
                .insert(entry.first, (entry.count, entry.sampled));
        }
        while self.records.len() > MAX_RECORDS {
            self.records.pop_first();
        }
        let gap = total > raw.len() as u64
            || entries.len() != raw.len()
            || uncertain
            || accounted != cursor - self.cursor;
        if gap {
            self.possible_gaps += 1;
            self.unknown_history |= self.scopes.is_empty();
            for scope in self.scopes.values_mut() {
                scope.gap();
            }
            self.version += 1;
        }
        self.cursor = cursor;
        for entry in entries {
            self.observe(entry, gap);
        }
    }

    fn observe(&mut self, entry: Entry, gap: bool) {
        let owned_fault = !compile_kind(&entry.diagnostic)
            && entry.diagnostic.code != ara_script::codes::COMPILE_RESULT
            && entry.diagnostic.data.as_deref().is_some_and(|data| {
                data["module"].as_str().is_some() || data.get("play").is_some()
            })
            && matches!(
                entry.diagnostic.severity,
                ara_diag::Severity::Error | ara_diag::Severity::Warning
            );
        if owned_fault {
            if entry
                .diagnostic
                .data
                .as_deref()
                .is_some_and(|data| data.get("compile").is_some())
            {
                self.invalid_metadata += 1;
            }
            if !self.protected.contains_key(&entry.first)
                && self.rows.len() + self.protected.len() >= MAX_ROWS
            {
                self.omitted_protected += 1;
            } else {
                self.protected.insert(entry.first, entry);
            }
            self.version += 1;
            return;
        }
        let Some(metadata) = Metadata::parse(&entry.diagnostic) else {
            if entry
                .diagnostic
                .data
                .as_deref()
                .is_some_and(|data| data.get("compile").is_some())
            {
                self.invalid_metadata += 1;
                self.version += 1;
            }
            return;
        };
        if metadata.phase == Phase::Diagnostic && !compile_kind(&entry.diagnostic) {
            self.invalid_metadata += 1;
            self.version += 1;
            return;
        }
        if !self.scopes.contains_key(&metadata.key) && self.scopes.len() >= MAX_SCOPES {
            self.omitted_scopes += 1;
            self.version += 1;
            return;
        }
        let mut scope = self.scopes.remove(&metadata.key).unwrap_or_default();
        scope.observe(self, &metadata, entry, gap);
        self.scopes.insert(metadata.key, scope);
        self.version += 1;
    }

    fn cache(&mut self, metadata: &Metadata, entry: Entry) -> bool {
        if !matches!(
            entry.diagnostic.severity,
            ara_diag::Severity::Error | ara_diag::Severity::Warning
        ) {
            return true;
        }
        let key = (metadata.key.clone(), metadata.order, entry.first);
        if !self.rows.contains_key(&key) && self.rows.len() + self.protected.len() >= MAX_ROWS {
            self.omitted_rows += 1;
            return false;
        }
        self.rows.insert(key, entry);
        true
    }

    fn retire_errors(&mut self, key: &Key, before: Order) {
        self.rows.retain(|(scope, order, _), entry| {
            scope != key
                || *order >= before
                || !entry.diagnostic.is_error()
                || !known_compile_kind(&entry.diagnostic)
        });
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.rows.values().chain(self.protected.values())
    }

    /// Unscoped/invalid provenance stays visible. Accepted or historical scoped
    /// records belong to the projection, avoiding duplicates from the bus tail.
    pub(super) fn fallback(&self, entry: &Entry) -> bool {
        if self.protected.contains_key(&entry.first) && entry.seq <= self.cursor {
            return false;
        }
        let Some(metadata) = Metadata::parse(&entry.diagnostic) else {
            return true;
        };
        if metadata.phase != Phase::Diagnostic || !compile_kind(&entry.diagnostic) {
            return true;
        }
        if let Some(scope) = self.scopes.get(&metadata.key)
            && scope
                .covered
                .is_some_and(|covered| metadata.order < covered)
            && entry.diagnostic.is_error()
            && known_compile_kind(&entry.diagnostic)
        {
            return false;
        }
        entry.seq > self.cursor || !self.scopes.contains_key(&metadata.key)
    }

    pub(super) fn notice(&self) -> Option<String> {
        let pending = self.scopes.values().filter(|scope| scope.pending()).count();
        let incomplete = self
            .scopes
            .values()
            .filter(|scope| scope.incomplete)
            .count();
        let uncertain = self.unknown_history
            || self.invalid_metadata > 0
            || self.scopes.values().any(Scope::omitted)
            || self.omitted_scopes > 0
            || self.omitted_protected > 0
            || self.read_error.is_some();
        if pending == 0 && incomplete == 0 && !uncertain {
            return None;
        }
        Some(format!(
            "C# compile state: {pending} pending, {incomplete} scopes without verified complete error coverage. Previous errors are retained until matching module inputs are verified. Warnings have partial coverage.{}{}",
            if uncertain {
                " Some diagnostics or provenance are unavailable; this view cannot establish a clean compile."
            } else {
                ""
            },
            self.read_error
                .as_ref()
                .map(|error| format!(" {error}"))
                .unwrap_or_default()
        ))
    }

    fn report(&self) -> Value {
        let scopes:Vec<_>=self.scopes.iter().map(|(key,scope)| {
            let rows:Vec<_>=self.rows.iter().filter(|((owner,_,_),_)|owner==key).map(|(_,entry)|entry).collect();
            json!({"key":key,"bound":scope.bound_report(),"order":scope.order(),"phase":scope.phase(),"incomplete":scope.incomplete,
                "covered":scope.covered,"pending":scope.pending(),"errors":rows.iter().filter(|entry|entry.diagnostic.is_error()).count(),
                "activeErrorSeqs":rows.iter().filter(|entry|entry.diagnostic.is_error()).map(|entry|entry.seq).collect::<Vec<_>>(),
                "lastVerifiedCompletion":scope.verified_report(),"unknown":scope.incomplete,
                "omittedCurrentRows":scope.omitted(),
                "warnings":rows.iter().filter(|entry|entry.diagnostic.severity==ara_diag::Severity::Warning).count(),
                "attempt":scope.attempt_report()})
        }).collect();
        json!({"schema":1,"projectionRevision":self.version,"polledPanelRevision":self.polled_revision,"cursor":self.cursor,"cachedRows":self.rows.len()+self.protected.len(),"scopeCount":self.scopes.len(),"possibleGaps":self.possible_gaps,
            "unknownEarlierScopes":self.unknown_history,"invalidMetadata":self.invalid_metadata,"omittedScopeEvents":self.omitted_scopes,
            "omittedRows":self.omitted_rows,"omittedProtectedRows":self.omitted_protected,
            "protectedRuntimeFaultSeqs":self.protected.values().map(|entry|entry.seq).collect::<Vec<_>>(),
            "readError":self.read_error,"notice":self.notice(),"scopes":scopes})
    }
}
