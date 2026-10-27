//! Read-only transaction projection; Undo/Redo keep the authority's author stacks.
use super::*;
use ara_ops::{Author, Store, Transaction, TxnId};
mod detail;
mod projection;
#[cfg(test)]
mod tests;

pub(super) const CSS: &str = include_str!("history_panel/history.css");
const PAGE: usize = 64;
const OPS: usize = 64;

/// Local reading position. Background transactions never replace a chosen detail.
#[derive(Default)]
pub(crate) struct View {
    selected: Option<TxnId>,
    anchor: Option<TxnId>,
    query: String,
}

impl Shell {
    /// Inspect a real immutable transaction without moving document selection.
    /// An empty target means the detail currently displayed in History.
    pub(crate) fn inspect_history(&mut self, editor: &mut Editor, txn: &str) -> ara_ui::Result<()> {
        let id = self
            .history_transaction(editor, txn)
            .ok_or("That transaction is not in this project's loaded history")?
            .id
            .clone();
        self.history_view.selected = Some(id);
        self.rebuild(editor)
    }

    /// Copy the complete canonical transaction, including every original op/value.
    pub(crate) fn copy_history_transaction(
        &mut self,
        editor: &Editor,
        txn: &str,
    ) -> ara_ui::Result<()> {
        let (text, message) = {
            let txn = self
                .history_transaction(editor, txn)
                .ok_or("That transaction is not in this project's loaded history")?;
            (
                ara_json::to_canonical_string(&serde_json::to_value(txn)?),
                format!(
                    "Copied transaction {} · {} operations",
                    txn.id,
                    txn.ops.len()
                ),
            )
        };
        self.pending_clipboard = Some(text);
        self.status_note = Some(message);
        Ok(())
    }

    fn history_transaction<'a>(
        &'a self,
        editor: &'a Editor,
        target: &str,
    ) -> Option<&'a Transaction> {
        let store = editor.host.store();
        if !target.is_empty() {
            return store.txn(&TxnId(target.to_owned()));
        }
        self.history_view
            .selected
            .as_ref()
            .and_then(|id| store.txn(id))
            .or_else(|| {
                let query = editor
                    .state
                    .searches
                    .get("history")
                    .map_or("", String::as_str)
                    .trim()
                    .to_lowercase();
                projection::page(store, &self.history_view, &query)
                    .rows
                    .first()
                    .copied()
            })
    }

    /// Page through the entire filtered log; each mounted page remains bounded.
    pub(crate) fn page_history(&mut self, editor: &mut Editor, older: bool) -> ara_ui::Result<()> {
        let query = editor
            .state
            .searches
            .get("history")
            .map_or("", String::as_str)
            .trim()
            .to_lowercase();
        let anchor = {
            let store = editor.host.store();
            let page = projection::page(store, &self.history_view, &query);
            let next = if older {
                if !page.older {
                    return Ok(());
                }
                page.start + PAGE
            } else {
                page.start.saturating_sub(PAGE)
            };
            if next == 0 {
                None
            } else {
                projection::matching(store, &query)
                    .nth(next)
                    .map(|txn| txn.id.clone())
            }
        };
        self.history_view.anchor = anchor;
        self.rebuild(editor)
    }

    pub(super) fn history_panel(&mut self, editor: &Editor, out: &mut String) {
        let query = editor
            .state
            .searches
            .get("history")
            .map_or("", String::as_str)
            .trim()
            .to_lowercase();
        if query != self.history_view.query {
            self.history_view.anchor = None;
            self.history_view.query.clone_from(&query);
        }
        let store = editor.host.store();
        let page = projection::page(store, &self.history_view, &query);
        out.push_str("<panel id='history-toolbar' class='toolbar history-toolbar'>");
        for (id, command, label, next) in [
            (
                "history-undo",
                "edit.undo",
                "Undo",
                store.next_undo(&Author::Human),
            ),
            (
                "history-redo",
                "edit.redo",
                "Redo",
                store.next_redo(&Author::Human),
            ),
        ] {
            self.bind_command(id, command, commands::Target::None);
            let why = next.map(projection::reason);
            let description = why.map_or_else(
                || format!("No human-author change to {}", label.to_lowercase()),
                |why| format!("{label} your latest change: {why}"),
            );
            out.push_str(&format!("<button id='{id}' class='ghost history-toolbar-action' text='{label}' description='{}' action='Dispatch'{} />",escape(&description),if next.is_some(){""}else{" disabled='true'"}));
        }
        self.search(out, "history", editor);
        for (id, command, label, enabled) in [
            ("history-newer", "history.newer", "Newer", page.start > 0),
            ("history-older", "history.older", "Older", page.older),
        ] {
            self.bind_command(id, command, commands::Target::None);
            out.push_str(&format!("<button id='{id}' class='ghost history-toolbar-action' text='{label}' action='Dispatch'{} />",if enabled{""}else{" disabled='true'"}));
        }
        out.push_str("</panel>");
        let count = if page.total == 0 {
            "No matching transactions".into()
        } else {
            format!(
                "{}–{} of {} matching transactions · newest first",
                page.start + 1,
                page.start + page.rows.len(),
                page.total
            )
        };
        out.push_str(&format!("<text id='history-page-count' class='caption history-count' text='{}' /><panel id='history-content' class='history-content'>",escape(&count)));
        if page.total == 0 {
            self.empty(out,"history-empty","clock",if store.history().is_empty(){"No content changes yet"}else{"No matching transactions"},"History records committed content changes. Search by author, reason, transaction ID or affected document; navigation has its own history.",&[]);
            out.push_str("</panel>");
            return;
        }
        let selected = self
            .history_view
            .selected
            .as_ref()
            .and_then(|id| store.txn(id))
            .or_else(|| page.rows.first().copied());
        out.push_str("<scroll id='history-transactions' class='history-transactions'>");
        for txn in &page.rows {
            let id = format!("history-row-{}", txn.id);
            self.bind_command(
                &id,
                "history.inspect",
                commands::Target::Text(txn.id.0.clone()),
            );
            let on = selected.is_some_and(|selected| selected.id == txn.id);
            let docs = projection::documents(store, txn);
            out.push_str(&format!("<button id='{id}' class='history-entry{}' label='Inspect transaction {}' description='{}' action='Dispatch'><text id='{id}-why' class='history-why history-entry-copy' text='{}' /><text id='{id}-meta' class='caption history-entry-copy' text='{} · r{} · {} operations' /><text id='{id}-documents' class='caption history-entry-copy' text='{}' /></button>",if on{" selected history-entry-on"}else{""},escape(&txn.id.0),escape(projection::reason(txn)),escape(&projection::short(projection::reason(txn),180)),escape(&txn.author.to_string()),txn.result,txn.ops.len(),escape(&docs)));
        }
        out.push_str("</scroll>");
        if let Some(txn) = selected {
            self.history_detail(store, txn, out);
        }
        out.push_str("</panel>");
    }
}
