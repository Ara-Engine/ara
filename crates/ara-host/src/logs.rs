//! The diagnostics bus behind `logs`: every subsystem's diagnostics in one bounded,
//! de-duplicated log with a cursor.
//!
//! Each entry has a sequence number. Repeating a diagnostic (same code, severity,
//! message and location) bumps its count and moves it to a new sequence number, so
//! `since: <cursor>` returns everything new or repeated since the last read.

use std::collections::{BTreeMap, HashMap};

use ara_diag::{Diagnostic, Severity};
use serde::Serialize;

/// Entries kept before the oldest is dropped.
pub const CAPACITY: usize = 4096;

/// One log entry.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Sequence number of the latest occurrence; pass it as `since` to read newer ones.
    pub seq: u64,
    /// How many times it occurred.
    pub count: u32,
    /// Sequence number of the first occurrence.
    pub first: u64,
    /// The diagnostic.
    pub diagnostic: Diagnostic,
}

/// A log filter.
#[derive(Debug, Default, Clone)]
pub struct LogFilter {
    /// Only entries after this sequence number.
    pub since: u64,
    /// Only these codes (`ARA1101`); empty means all.
    pub codes: Vec<String>,
    /// Minimum severity.
    pub severity: Option<Severity>,
    /// Only entries whose record is this entity or inside it.
    pub entity: Option<String>,
    /// Maximum entries.
    pub limit: usize,
}

/// The bounded, de-duplicated log.
#[derive(Debug, Default)]
pub struct Logs {
    entries: BTreeMap<u64, LogEntry>,
    by_key: HashMap<String, u64>,
    next: u64,
}

fn key(d: &Diagnostic) -> String {
    format!("{}|{}|{}|{}", d.code, d.severity, d.message, d.location)
}

impl Logs {
    /// Records a diagnostic; returns its entry.
    pub fn push(&mut self, d: Diagnostic) -> &LogEntry {
        self.next += 1;
        let seq = self.next;
        let k = key(&d);
        let (count, first) = match self.by_key.get(&k).and_then(|s| self.entries.remove(s)) {
            Some(old) => (old.count + 1, old.first),
            None => (1, seq),
        };
        self.by_key.insert(k, seq);
        while self.entries.len() >= CAPACITY {
            if let Some((_, old)) = self.entries.pop_first() {
                self.by_key.remove(&key(&old.diagnostic));
            }
        }
        self.entries.entry(seq).or_insert(LogEntry {
            seq,
            count,
            first,
            diagnostic: d,
        })
    }

    /// The newest sequence number (the cursor to read from next).
    pub fn cursor(&self) -> u64 {
        self.next
    }

    /// Entries matching `f`, oldest first.
    pub fn query(&self, f: &LogFilter) -> Vec<&LogEntry> {
        let rank = |s: Severity| match s {
            Severity::Error => 2,
            Severity::Warning => 1,
            Severity::Info => 0,
        };
        self.entries
            .range(f.since + 1..)
            .map(|(_, e)| e)
            .filter(|e| {
                let d = &e.diagnostic;
                (f.codes.is_empty() || f.codes.iter().any(|c| *c == d.code.to_string()))
                    && f.severity.is_none_or(|min| rank(d.severity) >= rank(min))
                    && f.entity.as_ref().is_none_or(|ent| {
                        d.location
                            .record
                            .as_ref()
                            .is_some_and(|r| r == ent || r.starts_with(&format!("{ent}/")))
                    })
            })
            .take(f.limit.max(1))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use ara_diag::codes;

    use super::*;

    #[test]
    fn repeats_are_counted_and_move_past_the_cursor() {
        let mut logs = Logs::default();
        let d = Diagnostic::warning(codes::DANGLING_ENTITY_REF, "dangling").record("e_aa");
        logs.push(d.clone());
        logs.push(Diagnostic::error(codes::IO, "disk").record("e_bb/e_cc"));
        let cursor = logs.cursor();
        assert!(
            logs.query(&LogFilter {
                since: cursor,
                limit: 10,
                ..Default::default()
            })
            .is_empty()
        );
        logs.push(d);
        let new = logs.query(&LogFilter {
            since: cursor,
            limit: 10,
            ..Default::default()
        });
        assert_eq!((new.len(), new[0].count, new[0].first), (1, 2, 1));

        let errors = logs.query(&LogFilter {
            severity: Some(Severity::Error),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(errors.len(), 1);
        let inside = logs.query(&LogFilter {
            entity: Some("e_bb".into()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(inside.len(), 1);
    }
}
