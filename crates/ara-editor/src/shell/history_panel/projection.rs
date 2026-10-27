use super::*;
use std::collections::BTreeSet;

pub(super) struct Page<'a> {
    pub rows: Vec<&'a Transaction>,
    pub start: usize,
    pub total: usize,
    pub older: bool,
}
pub(super) fn reason(txn: &Transaction) -> &str {
    txn.why
        .as_deref()
        .filter(|why| !why.trim().is_empty())
        .or(txn.label.as_deref())
        .unwrap_or("Content change")
}
pub(super) fn short(value: &str, limit: usize) -> String {
    let mut characters = value.chars();
    let mut text: String = characters.by_ref().take(limit).collect();
    if characters.next().is_some() {
        text.push('…');
    }
    text
}
pub(super) fn document<'a>(store: &'a Store, op: &'a ara_doc::Op) -> &'a str {
    if let ara_doc::Op::Document { path, .. } = op {
        return path;
    }
    store
        .workspace()
        .path_of(op.doc().as_str())
        .unwrap_or(op.doc().as_str())
}
pub(super) fn documents(store: &Store, txn: &Transaction) -> String {
    let paths: BTreeSet<_> = txn.ops.iter().map(|op| document(store, op)).collect();
    let mut text = paths
        .iter()
        .take(3)
        .map(|path| short(path, 100))
        .collect::<Vec<_>>()
        .join(" · ");
    if paths.len() > 3 {
        text.push_str(&format!(" · {} more documents", paths.len() - 3));
    }
    text
}
pub(super) fn matches(store: &Store, txn: &Transaction, query: &str) -> bool {
    query.is_empty()
        || txn.id.0.contains(query)
        || txn.author.to_string().to_lowercase().contains(query)
        || txn
            .why
            .as_ref()
            .is_some_and(|why| why.to_lowercase().contains(query))
        || txn
            .label
            .as_ref()
            .is_some_and(|label| label.to_lowercase().contains(query))
        || txn.ops.iter().any(|op| {
            document(store, op).to_lowercase().contains(query) || op.id().as_str().contains(query)
        })
}
pub(super) fn matching<'a>(store: &'a Store, query: &str) -> impl Iterator<Item = &'a Transaction> {
    store
        .history()
        .iter()
        .rev()
        .filter(move |txn| matches(store, txn, query))
}
pub(super) fn page<'a>(store: &'a Store, view: &View, query: &str) -> Page<'a> {
    let mut total = 0;
    let mut start = 0;
    for txn in matching(store, query) {
        if view.query == query && view.anchor.as_ref() == Some(&txn.id) {
            start = total;
        }
        total += 1;
    }
    let rows = matching(store, query)
        .skip(start)
        .take(PAGE)
        .collect::<Vec<_>>();
    let older = start + rows.len() < total;
    Page {
        rows,
        start,
        total,
        older,
    }
}
