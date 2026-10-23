use super::panels::display;
use super::*;
impl Shell {
    pub(super) fn console_panel(&mut self, editor: &mut Editor, out: &mut String) {
        out.push_str("<panel id='console-toolbar' class='toolbar'>");
        self.search(out, "console", editor);
        out.push_str("<text id='console-hint' class='muted' text='Filter by code, message or severity' /></panel><scroll id='console-scroll' class='body'>");
        let logs = editor
            .action("logs", json!({"limit":300}))
            .unwrap_or(Value::Null);
        let query = editor
            .state
            .searches
            .get("console")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        if let Some(entries) = logs["entries"].as_array() {
            for (index, entry) in entries.iter().enumerate() {
                let Ok(diagnostic) =
                    serde_json::from_value::<ara_diag::Diagnostic>(entry["diagnostic"].clone())
                else {
                    continue;
                };
                let label = format!(
                    "{}  {}  ×{}",
                    diagnostic.code, diagnostic.message, entry["count"]
                );
                if !query.is_empty()
                    && !format!("{} {:?}", label, diagnostic.severity)
                        .to_lowercase()
                        .contains(&query)
                {
                    continue;
                }
                out.push_str(&format!("<panel id='diagnostic-{index}' class='row'>"));
                out.push_str(&self.bind_button(
                    &format!("diagnostic-open-{index}"),
                    &label,
                    Binding::Diagnostic(diagnostic.clone()),
                    "quiet",
                ));
                for (fix_index, fix) in diagnostic.fixes.iter().enumerate() {
                    out.push_str(&self.bind_button(
                        &format!("fix-{index}-{fix_index}"),
                        &fix.title,
                        Binding::Fix(diagnostic.clone(), fix_index),
                        "small",
                    ));
                }
                out.push_str("</panel>");
            }
        }
        out.push_str("</scroll>");
    }
    pub(super) fn agents_panel(&mut self, editor: &mut Editor, out: &mut String) {
        out.push_str("<panel id='agents-toolbar' class='toolbar'><text id='policy-label' class='muted' text='Apply agent changes' />");
        for (policy, label) in [
            (crate::state::ApplyPolicy::Immediately, "Immediately"),
            (crate::state::ApplyPolicy::WhenIdle, "When idle"),
            (crate::state::ApplyPolicy::OnApproval, "On approval"),
        ] {
            out.push_str(&self.bind_button(
                &format!("policy-{policy:?}"),
                label,
                Binding::Protocol("editor.policy", json!({"policy":policy})),
                if editor.state.policy == policy {
                    "small selected"
                } else {
                    "small quiet"
                },
            ));
        }
        out.push_str("</panel><scroll id='agents-scroll' class='body'>");
        for (id, presence) in &editor.state.presence {
            out.push_str(&format!("<panel id='presence-{id}' class='row'><text id='presence-label-{id}' class='accent' text='{} / {} selected' />",escape(&presence.author),presence.selection.len()));
            out.push_str(&self.bind_button(
                &format!("follow-{id}"),
                if editor.state.follow.as_ref() == Some(id) {
                    "Stop following"
                } else {
                    "Follow"
                },
                Binding::Protocol(
                    "editor.follow",
                    json!({"agent":if editor.state.follow.as_ref()==Some(id){None}else{Some(id)}}),
                ),
                "small quiet",
            ));
            out.push_str("</panel>");
        }
        if let Some(pending) = editor.pending().as_array() {
            for item in pending {
                let ticket = item["ticket"].as_str().unwrap_or("");
                out.push_str(&format!("<panel id='pending-{ticket}' class='row'><text id='pending-label-{ticket}' class='warning' text='Waiting: {}' />",escape(item["why"].as_str().unwrap_or("Agent batch"))));
                out.push_str(&self.bind_button(
                    &format!("approve-{ticket}"),
                    "Apply",
                    Binding::Approve(ticket.into()),
                    "small",
                ));
                out.push_str(&self.bind_button(
                    &format!("reject-{ticket}"),
                    "Reject",
                    Binding::Reject(ticket.into()),
                    "small quiet",
                ));
                out.push_str("</panel>");
            }
        }
        let review = editor
            .action("review.list", json!({"limit":100}))
            .unwrap_or(Value::Null);
        if let Some(items) = review["items"].as_array() {
            for item in items {
                let Some(id) = item["txn"].as_str() else {
                    continue;
                };
                out.push_str(&format!("<panel id='turn-{id}' class='section'><panel id='turn-header-{id}' class='row'><text id='turn-title-{id}' text='{} / {}' /><panel id='turn-fill-{id}' style='flex-grow:1' />",escape(item["author"].as_str().unwrap_or("Agent")),escape(id)));
                out.push_str(&self.bind_button(
                    &format!("accept-{id}"),
                    "Accept",
                    Binding::Protocol("review.accept", json!({"txn":id})),
                    "small",
                ));
                out.push_str(&self.bind_button(
                    &format!("revert-{id}"),
                    "Revert",
                    Binding::Protocol("review.revert", json!({"txn":id})),
                    "small quiet",
                ));
                out.push_str("</panel>");
                out.push_str(&format!("<text id='turn-why-{id}' text='{}' /><text id='turn-commands-{id}' class='muted' text='{}' />",escape(item["why"].as_str().unwrap_or("No reason supplied")),escape(item["label"].as_str().unwrap_or("Commands"))));
                if let Some(tx) = editor.host.store().txn(&ara_ops::TxnId(id.into())) {
                    for (index, op) in tx.ops.iter().enumerate() {
                        let text = match op {
                            ara_doc::Op::Set {
                                id,
                                path,
                                prev,
                                value,
                                ..
                            } => format!(
                                "{id} / {path}   {} -> {}",
                                display(prev.as_ref().unwrap_or(&Value::Null)),
                                display(value.as_ref().unwrap_or(&Value::Null))
                            ),
                            ara_doc::Op::Create { id, .. } => format!("+ {id}"),
                            ara_doc::Op::Delete { id, .. } => format!("− {id}"),
                            ara_doc::Op::Move { id, parent, .. } => format!("{id} -> {parent:?}"),
                        };
                        out.push_str(&format!(
                            "<text id='turn-diff-{id}-{index}' class='warning' text='{}' />",
                            escape(&text)
                        ));
                    }
                }
                out.push_str("</panel>");
            }
        }
        out.push_str("</scroll>");
    }
}
