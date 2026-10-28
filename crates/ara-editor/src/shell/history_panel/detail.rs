use super::*;

fn value(value: Option<&Value>) -> String {
    match value {
        None => "unset".into(),
        Some(Value::String(text)) => projection::short(text, 160),
        Some(Value::Array(items)) => format!("Array · {} items", items.len()),
        Some(Value::Object(fields)) => format!("Object · {} fields", fields.len()),
        Some(value) => value.to_string(),
    }
}
fn description(op: &ara_doc::Op) -> String {
    match op {
        ara_doc::Op::File {
            path, state, prev, ..
        } => {
            let operation = match (prev.is_some(), state.is_some()) {
                (false, true) => "Create",
                (true, false) => "Delete",
                _ => "Replace",
            };
            format!("{operation} file entry · {path}")
        }
        ara_doc::Op::Set {
            path,
            prev,
            value: next,
            ..
        } => format!(
            "Set {path}: {} → {}",
            value(prev.as_ref()),
            value(next.as_ref())
        ),
        ara_doc::Op::Create { record, .. } => format!(
            "Create record · {}",
            record
                .get("name")
                .map(|name| value(Some(name)))
                .unwrap_or_else(|| format!("{} fields", record.len()))
        ),
        ara_doc::Op::Delete { snapshot, .. } => format!(
            "Delete record · {}",
            snapshot
                .get("name")
                .map(|name| value(Some(name)))
                .unwrap_or_else(|| format!("{} fields", snapshot.len()))
        ),
        ara_doc::Op::Move {
            parent,
            prev_parent,
            order,
            prev_order,
            ..
        } => format!(
            "Move parent {} → {}; order {} → {}",
            prev_parent.as_deref().unwrap_or("root"),
            parent.as_deref().unwrap_or("root"),
            prev_order
                .as_ref()
                .map_or("unchanged".into(), ToString::to_string),
            order
                .as_ref()
                .map_or("unchanged".into(), ToString::to_string)
        ),
        ara_doc::Op::Document { kind, prev, .. } => format!(
            "Document kind {} → {}",
            prev.as_deref().unwrap_or("absent"),
            kind.as_deref().unwrap_or("absent")
        ),
    }
}
impl Shell {
    pub(super) fn history_detail(&mut self, store: &Store, txn: &Transaction, out: &mut String) {
        self.bind_command(
            "history-copy",
            "history.copy",
            commands::Target::Text(txn.id.0.clone()),
        );
        out.push_str(&format!("<scroll id='history-detail' class='history-detail'><panel id='history-detail-head' class='history-detail-head'><text id='history-selected-id' class='mono history-detail-copy' text='{}' /><button id='history-copy' class='ghost' text='Copy full transaction' description='Copy canonical JSON including every operation and its exact values' action='Dispatch' /></panel><text id='history-detail-reason' class='history-reason history-detail-copy' text='{}' /><text id='history-detail-author' class='caption history-detail-copy' text='{} · revision {} → {} · {} operations' />",escape(&txn.id.0),escape(&projection::short(projection::reason(txn),2048)),escape(&txn.author.to_string()),txn.base,txn.result,txn.ops.len()));
        if let Some(label) = &txn.label {
            out.push_str(&format!("<text id='history-detail-label' class='caption history-detail-copy' text='Command: {}' />",escape(&projection::short(label,512))));
        }
        if let Some(target) = &txn.reverts {
            out.push_str(&format!("<text id='history-detail-reverts' class='caption history-detail-copy' text='Reverts {}' />",escape(&target.0)));
        }
        if let Some(reversal) = store.reverted_by(&txn.id) {
            out.push_str(&format!("<text id='history-detail-reverted-by' class='caption history-detail-copy' text='Reverted by {}' />",escape(&reversal.0)));
        }
        out.push_str("<text id='history-session-note' class='caption history-detail-copy' text='Undo and Redo use the current human author&apos;s stack. Selecting a transaction only inspects it.' />");
        for (index, op) in txn.ops.iter().enumerate().take(OPS) {
            out.push_str(&format!("<panel id='history-op-{index}' class='history-op'><text id='history-op-{index}-location' class='caption history-detail-copy' text='{} · {}' /><text id='history-op-{index}-change' class='history-detail-copy' text='{}' /></panel>",escape(projection::document(store,op)),escape(op.id().as_str()),escape(&description(op))));
        }
        if txn.ops.len() > OPS {
            out.push_str(&format!("<text id='history-more-ops' class='caption history-detail-copy' text='Showing the first {OPS} of {} operations. Copy full transaction for every operation and exact value.' />",txn.ops.len()));
        }
        out.push_str("</scroll>");
    }
}
