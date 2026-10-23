//! The host-side end of the bus: a bounded, de-duplicated store of diagnostics
//! with a cursor, causal links and queries by subject, code, severity, time and
//! origin.
//!
//! Repeating a diagnostic (same dedup key: code, severity, location, subjects and
//! message, unless the emitter gave an explicit key) bumps its count and moves it
//! to a new sequence number, so `since: <cursor>` returns everything new or repeated
//! since the last read. Retention is per severity, so info chatter never evicts
//! errors.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::bus::{Event, EventId, Lost, Meta, Origin, fnv};
use crate::{Code, Diagnostic, Severity, Subject, codes};

/// How many records of each severity are kept, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Error records kept before the oldest error is dropped.
    pub errors: usize,
    /// Warning records kept.
    pub warnings: usize,
    /// Info records kept.
    pub info: usize,
    /// Records not seen for this long are dropped on [`Store::expire`].
    pub max_age: Option<Duration>,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            errors: 2048,
            warnings: 2048,
            info: 1024,
            max_age: None,
        }
    }
}

/// One de-duplicated diagnostic in the store.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// Sequence number of the latest occurrence; pass it as `since` to read newer ones.
    pub seq: u64,
    /// Sequence number of the first occurrence.
    pub first: u64,
    /// Occurrences, including sampled ones.
    pub count: u32,
    /// Occurrences only counted, because sampling held them back.
    #[serde(skip_serializing_if = "is_zero")]
    pub sampled: u32,
    /// Event ID of the latest occurrence; other diagnostics name it as their `cause`.
    pub id: EventId,
    /// The event that caused the latest occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<EventId>,
    /// Who caused the latest occurrence.
    pub origin: Origin,
    /// Unix time of the latest occurrence, in milliseconds.
    pub at: u64,
    /// Unix time of the first occurrence, in milliseconds.
    pub first_at: u64,
    /// Simulation tick of the latest occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    /// The diagnostic.
    pub diagnostic: Diagnostic,
    #[serde(skip)]
    key: u64,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Record {
    /// Explicit subjects plus those implied by the location (its record as an
    /// entity, its document).
    pub fn subjects(&self) -> Vec<Subject> {
        let d = &self.diagnostic;
        let mut out = d.subjects.clone();
        let loc = &d.location;
        if let Some(r) = &loc.record
            && r.starts_with("e_")
        {
            out.push(Subject::entity(r.clone()));
        }
        match (&loc.file, &loc.doc) {
            (Some(file), Some(doc)) => out.push(Subject::document(format!("@{file}#{doc}"))),
            (Some(file), None) => out.push(Subject::document(file.clone())),
            (None, Some(doc)) => out.push(Subject::document(doc.clone())),
            (None, None) => {}
        }
        out
    }

    /// True if the record is about any of `subjects` (empty matches everything).
    pub fn about(&self, subjects: &[Subject]) -> bool {
        subjects.is_empty() || {
            let mine = self.subjects();
            subjects.iter().any(|q| mine.iter().any(|s| q.matches(s)))
        }
    }
}

/// Which origins a query wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginFilter {
    /// Only the engine's own diagnostics.
    Engine,
    /// Only diagnostics caused by humans.
    Human,
    /// Only diagnostics caused by agents (any agent).
    Agents,
    /// Only diagnostics caused by this agent.
    Agent(String),
}

impl OriginFilter {
    /// Parses `engine`, `human`, `agent` or `agent:<name>`.
    pub fn parse(text: &str) -> Option<OriginFilter> {
        Some(match text {
            "engine" => OriginFilter::Engine,
            "human" => OriginFilter::Human,
            "agent" | "agents" => OriginFilter::Agents,
            other => OriginFilter::Agent(other.strip_prefix("agent:")?.to_owned()),
        })
    }

    fn accepts(&self, origin: &Origin) -> bool {
        match (self, origin) {
            (OriginFilter::Engine, Origin::Engine)
            | (OriginFilter::Human, Origin::Human)
            | (OriginFilter::Agents, Origin::Agent(_)) => true,
            (OriginFilter::Agent(want), Origin::Agent(name)) => **name == **want,
            _ => false,
        }
    }
}

/// A store query. The default matches everything (up to `limit`, 0 = no limit).
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only records whose latest occurrence is after this sequence number.
    pub since: u64,
    /// Only these codes; empty means all.
    pub codes: Vec<Code>,
    /// Minimum severity.
    pub severity: Option<Severity>,
    /// Only records about one of these subjects; empty means all.
    pub subjects: Vec<Subject>,
    /// Only these origins.
    pub origin: Option<OriginFilter>,
    /// Only records last seen at or after this Unix time (ms).
    pub from_ms: Option<u64>,
    /// Only records last seen at or before this Unix time (ms).
    pub to_ms: Option<u64>,
    /// Only records last seen at or after this tick.
    pub from_tick: Option<u64>,
    /// Only records whose message contains this text (case-insensitive).
    pub text: Option<String>,
    /// Maximum records (the newest are kept when more match); 0 = all.
    pub limit: usize,
}

/// Severity rank: errors highest.
pub fn rank(s: Severity) -> u8 {
    match s {
        Severity::Error => 2,
        Severity::Warning => 1,
        Severity::Info => 0,
    }
}

impl Filter {
    /// True if `r` passes every condition except `since` and `limit`.
    pub fn accepts(&self, r: &Record) -> bool {
        let d = &r.diagnostic;
        (self.codes.is_empty() || self.codes.contains(&d.code))
            && self
                .severity
                .is_none_or(|min| rank(d.severity) >= rank(min))
            && self.origin.as_ref().is_none_or(|o| o.accepts(&r.origin))
            && self.from_ms.is_none_or(|t| r.at >= t)
            && self.to_ms.is_none_or(|t| r.at <= t)
            && self
                .from_tick
                .is_none_or(|t| r.tick.is_some_and(|rt| rt >= t))
            && self.text.as_ref().is_none_or(|t| {
                d.message.to_lowercase().contains(&t.to_lowercase())
            })
            && r.about(&self.subjects)
    }
}

/// The bounded, de-duplicated store.
#[derive(Debug)]
pub struct Store {
    records: BTreeMap<u64, Record>,
    by_key: HashMap<u64, u64>,
    by_severity: [BTreeSet<u64>; 3],
    /// Event IDs → dedup keys, for causal links; bounded FIFO.
    ids: HashMap<EventId, u64>,
    id_order: VecDeque<EventId>,
    next: u64,
    next_local: u32,
    retention: Retention,
    epoch: Instant,
    epoch_ms: u64,
}

impl Default for Store {
    fn default() -> Self {
        Store::new(Retention::default())
    }
}

fn dedup_key(d: &Diagnostic) -> u64 {
    let subjects: Vec<String> = d.subjects.iter().map(ToString::to_string).collect();
    let text = format!(
        "{}|{}|{}|{}|{}",
        d.code,
        d.severity,
        d.location,
        subjects.join(","),
        d.message
    );
    fnv(text.as_bytes())
}

impl Store {
    /// An empty store.
    pub fn new(retention: Retention) -> Store {
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        Store {
            records: BTreeMap::new(),
            by_key: HashMap::new(),
            by_severity: Default::default(),
            ids: HashMap::new(),
            id_order: VecDeque::new(),
            next: 0,
            next_local: 0,
            retention,
            epoch: Instant::now(),
            epoch_ms,
        }
    }

    fn unix_ms(&self, at: Instant) -> u64 {
        let since = at.saturating_duration_since(self.epoch).as_millis() as u64;
        let before = self.epoch.saturating_duration_since(at).as_millis() as u64;
        (self.epoch_ms + since).saturating_sub(before)
    }

    fn cap(&self, s: Severity) -> usize {
        match s {
            Severity::Error => self.retention.errors,
            Severity::Warning => self.retention.warnings,
            Severity::Info => self.retention.info,
        }
        .max(1)
    }

    /// Records a diagnostic logged directly on the consumer's thread (no ring).
    pub fn push(&mut self, d: Diagnostic, meta: Meta) -> &Record {
        let id = EventId(u64::from(self.next_local));
        self.next_local = self.next_local.wrapping_add(1);
        let tick = meta.tick.or_else(crate::bus::tick);
        let seq = self.insert(Event {
            id,
            cause: meta.cause,
            dedup: meta.dedup,
            origin: meta.origin.unwrap_or_default(),
            tick,
            at: Instant::now(),
            diagnostic: d,
        });
        &self.records[&seq]
    }

    /// Records drained events (ordered by emit time); returns the sequence numbers
    /// of the records they created or updated, in order.
    pub fn ingest(&mut self, mut events: Vec<Event>) -> Vec<u64> {
        events.sort_by_key(|e| e.at);
        events.into_iter().map(|e| self.insert(e)).collect()
    }

    /// Accounts for what a drain lost: sampled counts are added to each code's
    /// latest record; dropped events become one `ARA2001` warning. Returns the
    /// sequence numbers it created or updated.
    pub fn absorb(&mut self, lost: &Lost) -> Vec<u64> {
        let mut touched = Vec::new();
        for (code, n) in &lost.sampled {
            let latest = self
                .records
                .iter()
                .rev()
                .find(|(_, r)| r.diagnostic.code == *code)
                .map(|(seq, _)| *seq);
            if let Some(seq) = latest
                && let Some(mut r) = self.records.remove(&seq)
            {
                self.by_severity[rank(r.diagnostic.severity) as usize].remove(&seq);
                self.next += 1;
                r.seq = self.next;
                r.count = r.count.saturating_add(*n);
                r.sampled = r.sampled.saturating_add(*n);
                self.by_key.insert(r.key, r.seq);
                self.by_severity[rank(r.diagnostic.severity) as usize].insert(r.seq);
                touched.push(r.seq);
                self.records.insert(r.seq, r);
            }
        }
        if lost.dropped > 0 {
            let d = Diagnostic::warning(
                codes::DIAG_DROPPED,
                format!(
                    "{} diagnostics were dropped because an emitting thread's ring was full; the host drains it every tick",
                    lost.dropped
                ),
            );
            let seq = self.push(d, Meta::default().dedup("bus:dropped")).seq;
            touched.push(seq);
        }
        touched
    }

    fn insert(&mut self, e: Event) -> u64 {
        let key = e.dedup.unwrap_or_else(|| dedup_key(&e.diagnostic));
        let at = self.unix_ms(e.at);
        self.next += 1;
        let seq = self.next;
        let previous = self.by_key.get(&key).and_then(|s| self.records.remove(s));
        let record = match previous {
            Some(mut r) => {
                self.by_severity[rank(r.diagnostic.severity) as usize].remove(&r.seq);
                r.seq = seq;
                r.count = r.count.saturating_add(1);
                r.id = e.id;
                r.cause = e.cause.or(r.cause);
                r.origin = e.origin;
                r.at = at;
                r.tick = e.tick.or(r.tick);
                r.diagnostic = e.diagnostic;
                r
            }
            None => Record {
                seq,
                first: seq,
                count: 1,
                sampled: 0,
                id: e.id,
                cause: e.cause,
                origin: e.origin,
                at,
                first_at: at,
                tick: e.tick,
                diagnostic: e.diagnostic,
                key,
            },
        };
        let severity = record.diagnostic.severity;
        self.by_key.insert(key, seq);
        self.remember(e.id, key);
        let band = &mut self.by_severity[rank(severity) as usize];
        band.insert(seq);
        let cap = self.cap(severity);
        while self.by_severity[rank(severity) as usize].len() > cap {
            let Some(old) = self.by_severity[rank(severity) as usize].pop_first() else {
                break;
            };
            if let Some(r) = self.records.remove(&old) {
                self.by_key.remove(&r.key);
            }
        }
        self.records.insert(seq, record);
        seq
    }

    fn remember(&mut self, id: EventId, key: u64) {
        if self.ids.insert(id, key).is_none() {
            self.id_order.push_back(id);
        }
        let bound = 4 * (self.retention.errors + self.retention.warnings + self.retention.info);
        while self.id_order.len() > bound {
            if let Some(old) = self.id_order.pop_front() {
                self.ids.remove(&old);
            }
        }
    }

    /// Drops records last seen longer ago than `Retention::max_age`.
    pub fn expire(&mut self) {
        let Some(age) = self.retention.max_age else {
            return;
        };
        let cutoff = self.unix_ms(Instant::now()).saturating_sub(age.as_millis() as u64);
        self.clear(&Filter {
            to_ms: Some(cutoff.saturating_sub(1)),
            ..Filter::default()
        });
    }

    /// The newest sequence number (the cursor to read from next).
    pub fn cursor(&self) -> u64 {
        self.next
    }

    /// Records kept.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True when nothing is kept.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// One record by sequence number.
    pub fn get(&self, seq: u64) -> Option<&Record> {
        self.records.get(&seq)
    }

    /// The record an event ID belongs to (any of its occurrences), if still kept.
    pub fn by_id(&self, id: EventId) -> Option<&Record> {
        let key = self.ids.get(&id)?;
        self.records.get(self.by_key.get(key)?)
    }

    /// The record that caused `r`, if known and kept.
    pub fn cause_of(&self, r: &Record) -> Option<&Record> {
        self.by_id(r.cause?)
    }

    /// The chain of causes of `r`, nearest first (stops at cycles and after 16).
    pub fn causes(&self, r: &Record) -> Vec<&Record> {
        let mut out: Vec<&Record> = Vec::new();
        let mut cur = self.cause_of(r);
        while let Some(c) = cur {
            if out.len() >= 16 || c.seq == r.seq || out.iter().any(|o| o.seq == c.seq) {
                break;
            }
            out.push(c);
            cur = self.cause_of(c);
        }
        out
    }

    /// Records that name `r` as their cause.
    pub fn effects(&self, r: &Record) -> Vec<&Record> {
        self.records
            .values()
            .filter(|e| e.cause.is_some_and(|c| self.by_id(c).is_some_and(|x| x.seq == r.seq)))
            .collect()
    }

    /// Records matching `f`, oldest first; when more than `f.limit` match, the
    /// newest `limit` are returned.
    pub fn query(&self, f: &Filter) -> Vec<&Record> {
        let mut out: Vec<&Record> = self
            .records
            .range(f.since + 1..)
            .map(|(_, r)| r)
            .filter(|r| f.accepts(r))
            .collect();
        if f.limit > 0 && out.len() > f.limit {
            out.drain(..out.len() - f.limit);
        }
        out
    }

    /// Removes the records matching `f` (ignoring `limit`); returns how many.
    pub fn clear(&mut self, f: &Filter) -> usize {
        let doomed: Vec<u64> = self
            .records
            .range(f.since + 1..)
            .filter(|(_, r)| f.accepts(r))
            .map(|(seq, _)| *seq)
            .collect();
        for seq in &doomed {
            if let Some(r) = self.records.remove(seq) {
                self.by_key.remove(&r.key);
                self.by_severity[rank(r.diagnostic.severity) as usize].remove(seq);
            }
        }
        doomed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Subject;

    fn warn(msg: &str) -> Diagnostic {
        Diagnostic::warning(codes::DANGLING_ENTITY_REF, msg)
    }

    #[test]
    fn repeats_are_counted_and_move_past_the_cursor() {
        let mut s = Store::default();
        s.push(warn("dangling").record("e_aa"), Meta::default());
        s.push(Diagnostic::error(codes::IO, "disk").record("e_bb/e_cc"), Meta::default());
        let cursor = s.cursor();
        assert!(s.query(&Filter { since: cursor, ..Filter::default() }).is_empty());
        s.push(warn("dangling").record("e_aa"), Meta::default());
        let new = s.query(&Filter { since: cursor, ..Filter::default() });
        assert_eq!((new.len(), new[0].count, new[0].first), (1, 2, 1));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn queries_by_subject_severity_origin_and_text() {
        let mut s = Store::default();
        let crate_mesh = Subject::asset("@meshes/crate.glb#ms_crate001");
        s.push(
            Diagnostic::error(codes::ASSET_IMPORT_FAILED, "no meshes").subject(crate_mesh.clone()),
            Meta {
                origin: Some(Origin::Agent("codex".into())),
                ..Meta::default()
            },
        );
        s.push(warn("dangling").record("e_bb/e_cc").file("levels/a.scene"), Meta::default());
        let q = |f: Filter| s.query(&f).len();
        assert_eq!(q(Filter { subjects: vec![Subject::asset("ms_crate001")], ..Filter::default() }), 1);
        assert_eq!(q(Filter { subjects: vec![Subject::entity("e_bb")], ..Filter::default() }), 1);
        assert_eq!(q(Filter { subjects: vec![Subject::document("levels/a.scene")], ..Filter::default() }), 1);
        assert_eq!(q(Filter { severity: Some(Severity::Error), ..Filter::default() }), 1);
        assert_eq!(q(Filter { origin: OriginFilter::parse("agent:codex"), ..Filter::default() }), 1);
        assert_eq!(q(Filter { origin: OriginFilter::parse("human"), ..Filter::default() }), 0);
        assert_eq!(q(Filter { text: Some("DANGLING".into()), ..Filter::default() }), 1);
        assert_eq!(q(Filter { codes: vec![codes::IO], ..Filter::default() }), 0);
    }

    #[test]
    fn retention_is_per_severity() {
        let mut s = Store::new(Retention {
            errors: 2,
            warnings: 2,
            info: 2,
            max_age: None,
        });
        s.push(Diagnostic::error(codes::IO, "kept"), Meta::default());
        for i in 0..10 {
            s.push(Diagnostic::info(codes::REF_FILLED, format!("note {i}")), Meta::default());
        }
        let errors = s.query(&Filter { severity: Some(Severity::Error), ..Filter::default() });
        assert_eq!(errors.len(), 1, "info chatter never evicts errors");
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn causes_link_records_and_survive_dedup() {
        let mut s = Store::default();
        let parent = s
            .push(Diagnostic::error(codes::JSON_SYNTAX, "bad material"), Meta::default())
            .id;
        let child = s
            .push(warn("mesh drawn without material"), Meta::caused_by(Some(parent)))
            .seq;
        s.push(Diagnostic::error(codes::JSON_SYNTAX, "bad material"), Meta::default());
        let child = s.get(child).expect("child kept");
        let causes = s.causes(child);
        assert_eq!(causes.len(), 1);
        assert_eq!(causes[0].count, 2, "the repeated parent is still the cause");
        assert_eq!(s.effects(causes[0]).len(), 1);
    }

    #[test]
    fn sampled_counts_join_the_latest_record_and_drops_are_reported() {
        let mut s = Store::default();
        s.push(warn("spam"), Meta::default());
        s.absorb(&Lost {
            dropped: 3,
            sampled: vec![(codes::DANGLING_ENTITY_REF, 40)],
        });
        let spam = s.query(&Filter { codes: vec![codes::DANGLING_ENTITY_REF], ..Filter::default() });
        assert_eq!((spam[0].count, spam[0].sampled), (41, 40));
        let dropped = s.query(&Filter { codes: vec![codes::DIAG_DROPPED], ..Filter::default() });
        assert!(dropped[0].diagnostic.message.starts_with("3 diagnostics"));
    }

    #[test]
    fn clear_removes_matches_only() {
        let mut s = Store::default();
        s.push(warn("a").record("e_aa"), Meta::default());
        s.push(warn("b").record("e_bb"), Meta::default());
        let removed = s.clear(&Filter {
            subjects: vec![Subject::entity("e_aa")],
            ..Filter::default()
        });
        assert_eq!((removed, s.len()), (1, 1));
        s.push(warn("a").record("e_aa"), Meta::default());
        assert_eq!(s.query(&Filter::default()).iter().map(|r| r.count).max(), Some(1));
    }
}
