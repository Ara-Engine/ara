use super::panels::display;
use super::*;
impl Shell {
    pub(super) fn console_panel(&mut self, editor: &mut Editor, out: &mut String) {
        out.push_str("<panel id='console-toolbar' class='toolbar'>");
        self.search(out, "console", editor);
        out.push_str(
            "</panel><scroll id='console-scroll' class='body' style='padding:4px;gap:0px'>",
        );
        let mut shown = 0;
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
                shown += 1;
                let (icon, tint) = match diagnostic.severity {
                    ara_diag::Severity::Error => ("circle-x", "destructive"),
                    ara_diag::Severity::Warning => ("triangle-alert", "warning"),
                    _ => ("info", "info"),
                };
                let open = format!("diagnostic-open-{index}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&open),
                    Binding::Diagnostic(diagnostic.clone()),
                );
                out.push_str(&format!("<panel id='diagnostic-{index}' class='row' style='min-height:22px;padding-left:4px;gap:6px;flex-shrink:0'>{}<button id='{open}' class='ghost' style='flex-grow:1;flex-basis:0px;justify-content:start;height:22px' label='{}' description='Select the entity and reveal the field' action='Dispatch'><text id='{open}-code' class='caption mono' text='{}' /><text id='{open}-text' class='fg' text='{}' /><panel id='{open}-fill' class='fill' /><text id='{open}-count' class='caption' text='×{}' /></button>",icons::icon_in(&open,icon,tint),escape(&label),escape(&diagnostic.code.to_string()),escape(&diagnostic.message),entry["count"]));
                for (fix_index, fix) in diagnostic.fixes.iter().enumerate() {
                    out.push_str(&self.bind_button(
                        &format!("fix-{index}-{fix_index}"),
                        &fix.title,
                        Binding::Fix(diagnostic.clone(), fix_index),
                        "primary",
                    ));
                }
                out.push_str("</panel>");
            }
        }
        if shown == 0 {
            out.push_str("</scroll>");
            self.empty(out, "console-empty", "circle-check", if query.is_empty() { "No problems" } else { "No matching messages" }, "Diagnostics from the engine, importers and agents appear here, with one-click fixes.", &[]);
            return;
        }
        out.push_str("</scroll>");
    }
    pub(super) fn agents_panel(&mut self, editor: &mut Editor, out: &mut String) {
        let compact = self.ui.size[1] < 800.;
        out.push_str("<panel id='agents-toolbar' class='toolbar'>");
        out.push_str(if compact {
            "</panel><scroll id='agents-scroll' class='body' style='padding:4px;gap:8px'>"
        } else {
            "</panel><scroll id='agents-scroll' class='body'>"
        });
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
        let mut review = editor
            .action("history.list", json!({"limit":100}))
            .unwrap_or(Value::Null);
        if let Some(items) = review["items"].as_array_mut() {
            items.retain(|item| {
                item["author"]
                    .as_str()
                    .is_some_and(|a| a.starts_with("agent:"))
                    && item["reverts"].is_null()
            });
        }
        let nothing = editor.state.presence.is_empty()
            && review["items"].as_array().is_none_or(Vec::is_empty);
        if nothing {
            out.push_str("</scroll>");
            self.empty(out, "agents-empty", "bot", "No agent activity yet", "Agents connected over MCP edit through the same commands as you. Their changes appear here with a reason and a diff.", &[("agents-empty-connect", "Connect an agent", "agents.connect")]);
            return;
        }
        let names =
            crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap_or_default();
        if let Some(items) = review["items"].as_array() {
            let mut shown_turns = std::collections::BTreeSet::new();
            for item in items {
                if let Some(turn) = item["turn"].as_str()
                    && !shown_turns.insert(turn)
                {
                    continue;
                }
                let Some(id) = item["txn"].as_str() else {
                    continue;
                };
                // Title, reason and actions stack so narrow docks keep every control reachable.
                out.push_str(&format!("<panel id='turn-{id}' class='agent-card'><panel id='turn-header-{id}' class='row'><panel id='avatar-{id}' class='avatar'>{}</panel><text id='turn-title-{id}' class='fg' text='{}' /><panel id='turn-fill-{id}' class='fill' /><text id='turn-meta-{id}' class='caption mono' text='{}' /></panel><text id='turn-why-{id}' class='dialog-text' style='color:var(--foreground)' text='{}' />",icons::icon(&format!("avatar-{id}"),"bot"),escape(item["author"].as_str().unwrap_or("Agent")),escape(&id.chars().take(8).collect::<String>()),escape(item["why"].as_str().unwrap_or("No reason supplied"))));
                if !compact {
                    out.push_str(&format!(
                        "<text id='turn-commands-{id}' class='caption' text='{}' />",
                        escape(item["label"].as_str().unwrap_or("Commands"))
                    ));
                }
                out.push_str(&format!(
                    "<panel id='turn-actions-{id}' class='row' style='gap:4px'>"
                ));

                if item["status"] == "reverted" {
                    out.push_str(&format!(
                        "<text id='undone-{id}' class='caption' text='Undone' />"
                    ));
                } else {
                    out.push_str(&self.bind_button(
                        &format!("revert-{id}"),
                        "Undo turn",
                        Binding::Protocol("review.revert", json!({"txn":id})),
                        "",
                    ));
                }
                out.push_str("</panel>");
                let transactions: Vec<_> = items
                    .iter()
                    .filter(|other| {
                        other["txn"] == item["txn"]
                            || (item["turn"].is_string() && other["turn"] == item["turn"])
                    })
                    .filter_map(|other| other["txn"].as_str())
                    .filter_map(|id| editor.host.store().txn(&ara_ops::TxnId(id.into())))
                    .collect();
                {
                    for (index, op) in transactions.iter().flat_map(|tx| tx.ops.iter()).enumerate()
                    {
                        let text = match op {
                            ara_doc::Op::Set {
                                id,
                                path,
                                prev,
                                value,
                                ..
                            } => format!(
                                "{} / {path}   {} -> {}",
                                names
                                    .get(id.as_str())
                                    .and_then(|e| e.record.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or(id.as_str()),
                                display(prev.as_ref().unwrap_or(&Value::Null)),
                                display(value.as_ref().unwrap_or(&Value::Null))
                            ),
                            ara_doc::Op::Create { id, .. } => format!("+ {id}"),
                            ara_doc::Op::Delete { id, .. } => format!("− {id}"),
                            ara_doc::Op::Move { id, parent, .. } => format!("{id} -> {parent:?}"),
                            ara_doc::Op::Document { path, kind, .. } => match kind {
                                Some(kind) => format!("+ {path} ({kind})"),
                                None => format!("− {path}"),
                            },
                        };
                        let class = match op {
                            ara_doc::Op::Create { .. } => "chip-success",
                            ara_doc::Op::Delete { .. } => "chip-destructive",
                            _ => "chip-warning",
                        };
                        out.push_str(&format!("<panel id='turn-diff-{id}-{index}' class='diff'><text id='diff-chip-{id}-{index}' class='{class}' text='{}' /></panel>",escape(&text)));
                    }
                }
                out.push_str("</panel>");
            }
        }
        out.push_str("</scroll>");
    }
}
