//! The on-disk journal (`.ara/journal`): an append-only JSON-lines log of committed
//! transactions, document flushes, sessions, checkpoints and review decisions.
//!
//! On open the host replays it: transactions newer than what each document's file
//! holds are re-applied (crash recovery); older ones only rebuild history, so undo
//! and review survive restarts. A document's on-disk revision is the newest `flush`
//! entry whose content hash matches the file.
//!
//! The journal is bounded: past [`MAX_ENTRIES`] it is rewritten with a `base` entry
//! and the newest [`KEEP_TXNS`] transactions (plus the entries they need); the
//! rewrite is synced to disk before it replaces the file.
//!
//! The host's file thread writes it, in the order the host commits, through the
//! same queue as document writes: the host thread never waits on the disk, a
//! document's file never gets ahead of its transaction here, and a crash leaves an
//! ordered prefix (a torn last line is skipped on open). A command is acknowledged
//! once its ops are applied in memory; each entry is flushed to the OS as soon as
//! the thread reaches it, so a process crash loses at most the entries still
//! queued. There is no fsync per entry: not safe against power loss.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use ara_diag::{Diagnostic, codes};
use ara_ops::Transaction;
use serde::{Deserialize, Serialize};

use crate::params::ReviewStatus;
use crate::session::{Checkpoint, Session};

/// Entries after which the journal is compacted.
pub const MAX_ENTRIES: usize = 4096;
/// Transactions kept by compaction.
pub const KEEP_TXNS: usize = 1024;
/// Journal format version, written in `base` entries.
pub const VERSION: u32 = 1;

/// One journal line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Entry {
    /// History before `revision` was compacted away.
    Base {
        /// The store revision the retained history starts after.
        revision: u64,
        /// Journal format version.
        version: u32,
    },
    /// A committed transaction and the session that made it.
    Txn {
        /// Session handle, if a session made it (external edits have none).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The transaction.
        txn: Transaction,
    },
    /// A document was written to disk at `rev`.
    Flush {
        /// Asset ID.
        doc: String,
        /// Store revision whose state was written.
        rev: u64,
        /// FNV-1a hash of the written text (see [`hash`]).
        hash: String,
    },
    /// A session began (with its approval decision).
    Session {
        /// The session as approved.
        session: Session,
    },
    /// A session ended.
    Ended {
        /// Session handle.
        session: String,
    },
    /// A checkpoint was taken.
    Checkpoint {
        /// Session handle.
        session: String,
        /// The checkpoint.
        checkpoint: Checkpoint,
    },
    /// A review decision (`entity: None` means the whole transaction).
    Review {
        /// Transaction ID.
        txn: String,
        /// Entity key (`sc_x/e_y`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
        /// New status.
        status: ReviewStatus,
    },
}

/// FNV-1a 64 of `text`, as 16 hex digits. Stable across runs and platforms.
pub fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The journal file, open for appending.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    file: File,
    entries: usize,
}

impl Journal {
    /// Opens (creating) `<state>/journal` and reads every entry. Unreadable lines are
    /// skipped and reported (`ARA1905`); a torn last line after a crash is one.
    pub fn open(state_dir: &Path) -> io::Result<(Journal, Vec<Entry>, Vec<Diagnostic>)> {
        fs::create_dir_all(state_dir)?;
        let path = state_dir.join("journal");
        let (entries, diags) = if path.exists() {
            read_entries(&path)?
        } else {
            (Vec::new(), Vec::new())
        };
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let journal = Journal {
            path,
            file,
            entries: entries.len(),
        };
        Ok((journal, entries, diags))
    }

    /// Reads every readable entry back from the file.
    pub fn read_all(&self) -> io::Result<Vec<Entry>> {
        read_entries(&self.path).map(|(e, _)| e)
    }

    /// Appends one entry and flushes it to the OS.
    pub fn append(&mut self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        self.entries += 1;
        Ok(())
    }

    /// Number of entries in the file.
    pub fn len(&self) -> usize {
        self.entries
    }

    /// True if the journal has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }

    /// Compacts the journal in place: reads it back, keeps what [`compact`] keeps
    /// ([`KEEP_TXNS`] transactions) and rewrites it.
    pub fn compact(&mut self) -> io::Result<()> {
        let entries = self.read_all()?;
        self.rewrite(&compact(&entries, KEEP_TXNS))
    }

    /// Rewrites the journal as `entries` (atomically: temp file, synced, then rename).
    pub fn rewrite(&mut self, entries: &[Entry]) -> io::Result<()> {
        let tmp = self.path.with_extension("tmp");
        {
            let mut out = File::create(&tmp)?;
            for e in entries {
                let mut line = serde_json::to_string(e).map_err(io::Error::other)?;
                line.push('\n');
                out.write_all(line.as_bytes())?;
            }
            out.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        self.file = OpenOptions::new().append(true).open(&self.path)?;
        self.entries = entries.len();
        Ok(())
    }
}

fn read_entries(path: &Path) -> io::Result<(Vec<Entry>, Vec<Diagnostic>)> {
    let mut entries = Vec::new();
    let mut diags = Vec::new();
    let reader = BufReader::new(File::open(path)?);
    for (n, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Entry>(&line) {
            Ok(e) => entries.push(e),
            Err(e) => diags.push(
                Diagnostic::warning(
                    codes::JOURNAL_CORRUPT,
                    format!("journal line {} skipped: {e}", n + 1),
                )
                .file(".ara/journal"),
            ),
        }
    }
    Ok((entries, diags))
}

/// The entries a compacted journal keeps: a `base`, the newest `keep` transactions,
/// the sessions, checkpoints and review entries they refer to, and the newest flush
/// per document.
pub fn compact(entries: &[Entry], keep: usize) -> Vec<Entry> {
    let txns: Vec<&Transaction> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Txn { txn, .. } => Some(txn),
            _ => None,
        })
        .collect();
    let first_kept = txns.len().saturating_sub(keep);
    let kept: BTreeSet<&str> = txns[first_kept..].iter().map(|t| t.id.0.as_str()).collect();
    let base = txns
        .get(first_kept)
        .map_or_else(|| last_revision(entries), |t| t.result.saturating_sub(1));
    let sessions: BTreeSet<&str> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Txn {
                session: Some(s),
                txn,
            } if kept.contains(txn.id.0.as_str()) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let mut last_flush: BTreeMap<&str, &Entry> = BTreeMap::new();
    for e in entries {
        if let Entry::Flush { doc, .. } = e {
            last_flush.insert(doc, e);
        }
    }
    let mut out = vec![Entry::Base {
        revision: base,
        version: VERSION,
    }];
    out.extend(last_flush.into_values().cloned());
    for e in entries {
        let keep = match e {
            Entry::Txn { txn, .. } => kept.contains(txn.id.0.as_str()),
            Entry::Session { session } => sessions.contains(session.id.as_str()),
            Entry::Ended { session } | Entry::Checkpoint { session, .. } => {
                sessions.contains(session.as_str())
            }
            Entry::Review { txn, .. } => kept.contains(txn.as_str()),
            Entry::Base { .. } | Entry::Flush { .. } => false,
        };
        if keep {
            out.push(e.clone());
        }
    }
    out
}

/// The newest revision mentioned by the entries.
fn last_revision(entries: &[Entry]) -> u64 {
    entries
        .iter()
        .map(|e| match e {
            Entry::Base { revision, .. } => *revision,
            Entry::Txn { txn, .. } => txn.result,
            Entry::Flush { rev, .. } => *rev,
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn txn(n: u64) -> Entry {
        Entry::Txn {
            session: Some(format!("ses_{n}")),
            txn: serde_json::from_value(json!({
                "id": format!("tx_{n}"), "author": "agent:a", "base": n - 1, "result": n, "ops": []
            }))
            .expect("transaction"),
        }
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(hash(""), "cbf29ce484222325");
        assert_ne!(hash("a"), hash("b"));
    }

    #[test]
    fn compaction_keeps_the_newest_transactions() {
        let mut entries: Vec<Entry> = (1..=5).map(txn).collect();
        entries.push(Entry::Flush {
            doc: "sc_a".into(),
            rev: 5,
            hash: "h".into(),
        });
        entries.push(Entry::Review {
            txn: "tx_1".into(),
            entity: None,
            status: ReviewStatus::Accepted,
        });
        let out = compact(&entries, 2);
        assert_eq!(
            out[0],
            Entry::Base {
                revision: 3,
                version: VERSION
            }
        );
        let ids: Vec<String> = out
            .iter()
            .filter_map(|e| match e {
                Entry::Txn { txn, .. } => Some(txn.id.0.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["tx_4", "tx_5"]);
        assert!(out.iter().any(|e| matches!(e, Entry::Flush { rev: 5, .. })));
        assert!(!out.iter().any(|e| matches!(e, Entry::Review { .. })));
    }
}
