//! Selection follows exact undo/redo chains only while its last navigation is intact.
use super::*;
use ara_ops::{Author, Transaction, TxnId};

struct Record {
    source: TxnId,
    head: TxnId,
    applied: bool,
    before: Selection,
    after: Selection,
}
struct Pending {
    expected: Selection,
    desired: Selection,
    began: Instant,
}

#[derive(Default)]
pub(super) struct History {
    revision: u64,
    records: Vec<Record>,
    pending: Option<Pending>,
}
impl History {
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn preserved_keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.pending.iter().flat_map(|pending| {
            pending
                .expected
                .keys
                .iter()
                .chain(&pending.desired.keys)
                .copied()
        })
    }
    pub(super) fn applied(&self, txn: &str) -> Option<bool> {
        self.records
            .iter()
            .find(|record| record.source.0 == txn)
            .map(|record| record.applied)
    }
    pub(super) fn remember(
        &mut self,
        txn: TxnId,
        revision: u64,
        before: Selection,
        after: Selection,
    ) {
        self.records.push(Record {
            source: txn.clone(),
            head: txn,
            applied: true,
            before,
            after,
        });
        // The acknowledgement may arrive after a native Undo. Replay that exact
        // chain rather than handing the browser a forward selection for an undo.
        self.revision = self.revision.min(revision);
    }
    fn revert(&mut self, transaction: &Transaction, current: impl FnOnce() -> Selection) -> bool {
        let Some(index) = self
            .records
            .iter()
            .position(|record| transaction.reverts.as_ref() == Some(&record.head))
        else {
            return false;
        };
        let current = current();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| !pending.expected.same_navigation(&current))
        {
            self.pending = None;
        }
        let record = &mut self.records[index];
        let (expected, desired) = if record.applied {
            (&record.after, &record.before)
        } else {
            (&record.before, &record.after)
        };
        let guarded = self
            .pending
            .as_ref()
            .map_or(&current, |pending| &pending.desired)
            .same_navigation(expected);
        if transaction.author == Author::Human && guarded {
            if let Some(pending) = &mut self.pending {
                pending.desired = desired.clone();
            } else {
                self.pending = Some(Pending {
                    expected: current,
                    desired: desired.clone(),
                    began: Instant::now(),
                });
            }
        } else {
            self.pending = None;
        }
        record.head = transaction.id.clone();
        record.applied = !record.applied;
        true
    }
}
impl Shell {
    /// Observe before pruning keys: an undo may remove the currently selected paths
    /// before the asynchronous semantic index publishes their restored names.
    pub(in crate::shell) fn observe_asset_history(&mut self, editor: &Editor) -> bool {
        let revision = editor.host.store().revision();
        if revision == self.asset_ops.history.revision {
            if self
                .asset_ops
                .history
                .pending
                .as_ref()
                .is_some_and(|pending| !pending.expected.unchanged(self, editor))
            {
                self.asset_ops.history.pending = None;
                self.asset_ops.revision += 1;
                return true;
            }
            return false;
        }
        let mut history = std::mem::take(&mut self.asset_ops.history);
        let mut changed = false;
        for transaction in editor.host.store().since(history.revision) {
            changed |= history.revert(transaction, || Selection::read(self, editor));
        }
        history.revision = revision;
        self.asset_ops.history = history;
        if changed {
            self.browser.refresh();
            self.asset_ops.revision += 1;
        }
        changed
    }
    pub(super) fn follow_asset_history(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(pending) = &self.asset_ops.history.pending else {
            return Ok(false);
        };
        if !pending.expected.unchanged(self, editor) {
            self.asset_ops.history.pending = None;
            return Ok(true);
        }
        let desired = &pending.desired;
        let folder_ready = desired.folder.is_empty()
            || (!editor.host.store().workspace().reference_root().is_empty()
                && desired.folder == editor.host.store().workspace().reference_root())
            || self
                .browser
                .items
                .iter()
                .any(|asset| asset.kind == "folder" && asset.path == desired.folder);
        if !folder_ready
            || !desired
                .paths
                .iter()
                .all(|path| self.browser.items.iter().any(|asset| &asset.path == path))
        {
            if pending.began.elapsed() > Duration::from_secs(30) {
                self.asset_ops.history.pending = None;
                self.status_note = Some(
                    "Asset history changed; the browser is still refreshing its entries".into(),
                );
                return Ok(true);
            }
            return Ok(false);
        }
        editor.action(
            "editor.workspace",
            json!({"folder":desired.folder,"asset":desired.asset.as_deref().unwrap_or("")}),
        )?;
        let pending = self
            .asset_ops
            .history
            .pending
            .take()
            .ok_or("Asset history navigation was removed")?;
        let first = pending
            .desired
            .active
            .filter(|key| pending.desired.keys.contains(key));
        self.asset_selection
            .set_selected(first.into_iter().chain(pending.desired.keys));
        if let Some(active) = pending.desired.active {
            self.asset_selection.focus(active);
        }
        self.asset_selection_path = pending.desired.primary;
        self.asset_grid_dirty = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selection(paths: &[&str]) -> Selection {
        let mut keys: Vec<_> = paths
            .iter()
            .map(|path| ara_ui::source::stable_id(path))
            .collect();
        keys.sort_unstable();
        let asset = paths.first().map(|path| (*path).to_owned());
        Selection {
            doc: "sc_assets".into(),
            entities: vec!["e_selected".into()],
            primary: asset.clone(),
            active: asset.as_deref().map(ara_ui::source::stable_id),
            asset,
            folder: "game/Art".into(),
            keys,
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }
    }
    fn transaction(id: &str, reverts: &str, author: Author) -> Transaction {
        Transaction {
            id: TxnId(id.into()),
            author,
            why: None,
            label: None,
            base: 0,
            result: 1,
            idempotency_key: None,
            reverts: Some(TxnId(reverts.into())),
            ops: Vec::new(),
        }
    }
    #[test]
    fn rapid_multilevel_undo_redo_composes_without_intermediate_navigation() {
        let before = selection(&["game/Art/A.prefab", "game/Art/B.prefab"]);
        let renamed = selection(&["game/Art/Renamed.prefab", "game/Art/B.prefab"]);
        let moved = selection(&["game/Archive/Renamed.prefab", "game/Archive/B.prefab"]);
        let mut history = History::default();
        history.remember(
            TxnId("tx_rename".into()),
            1,
            before.clone(),
            renamed.clone(),
        );
        history.remember(TxnId("tx_move".into()), 2, renamed, moved.clone());
        for (id, reverts) in [("tx_undo_move", "tx_move"), ("tx_undo_rename", "tx_rename")] {
            assert!(history.revert(&transaction(id, reverts, Author::Human), || moved.clone()));
        }
        assert!(
            history
                .pending
                .as_ref()
                .is_some_and(|pending| pending.expected == moved && pending.desired == before)
        );
        for (id, reverts) in [
            ("tx_redo_rename", "tx_undo_rename"),
            ("tx_redo_move", "tx_undo_move"),
        ] {
            assert!(history.revert(&transaction(id, reverts, Author::Human), || moved.clone()));
        }
        assert!(
            history
                .pending
                .as_ref()
                .is_some_and(|pending| pending.expected == moved && pending.desired == moved)
        );
    }
    #[test]
    fn history_does_not_replace_a_later_human_selection_or_follow_an_agent() {
        let original = selection(&["game/Art/A.prefab", "game/Art/B.prefab"]);
        let copies = selection(&["game/Art/A Copy.prefab", "game/Art/B Copy.prefab"]);
        let human = selection(&["game/Art/A.prefab"]);
        let mut history = History::default();
        history.remember(TxnId("tx_copy".into()), 1, original.clone(), copies.clone());
        assert!(
            history.revert(&transaction("tx_undo", "tx_copy", Author::Human), || human
                .clone())
        );
        assert!(!history.pending());
        assert!(history.revert(&transaction("tx_redo", "tx_undo", Author::Human), || human));
        assert!(!history.pending());
        assert!(history.revert(
            &transaction(
                "tx_agent_undo",
                "tx_redo",
                Author::Agent("automation".into())
            ),
            || copies
        ));
        assert!(!history.pending());
        assert!(history.revert(
            &transaction("tx_human_redo", "tx_agent_undo", Author::Human),
            || original
        ));
        assert!(history.pending());
    }
    #[test]
    fn an_undo_before_job_acknowledgement_still_retains_its_redo_mapping() {
        let before = selection(&["game/Art/A.prefab", "game/Art/B.prefab"]);
        let after = selection(&["game/Archive/A.prefab", "game/Archive/B.prefab"]);
        let mut history = History {
            revision: 12,
            ..History::default()
        };
        history.remember(TxnId("tx_move".into()), 10, before.clone(), after.clone());
        assert_eq!(history.revision, 10);
        assert!(history.revert(
            &transaction("tx_early_undo", "tx_move", Author::Human),
            || before.clone()
        ));
        assert_eq!(history.applied("tx_move"), Some(false));
        assert!(!history.pending());
        assert!(history.revert(
            &transaction("tx_redo", "tx_early_undo", Author::Human),
            || before
        ));
        assert_eq!(history.applied("tx_move"), Some(true));
        assert!(
            history
                .pending
                .as_ref()
                .is_some_and(|pending| pending.desired == after)
        );
    }
}
