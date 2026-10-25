//! The Agents panel as an activity feed (D24, amended): who did what, when and why,
//! with undo per agent turn. Agents' edits already applied; nothing here asks for
//! approval. Connected agents sit on top with View (jump once) and Follow.
use super::commands::Target;
use super::panels::display;
use super::*;
use crate::activity::short_author;

/// One agent turn in the feed: its transactions, newest first.
struct Turn {
    key: String,
    author: String,
    why: String,
    txns: Vec<ara_ops::TxnId>,
    undone: bool,
}

impl Shell {
    pub(super) fn agents_panel(&mut self, editor: &mut Editor, out: &mut String) {
        out.push_str("<panel id='agents-toolbar' class='toolbar'>");
        if editor.state.presence.is_empty() {
            out.push_str("<text id='agents-none' class='caption' text='No agent connected' />");
        }
        for (id, presence) in &editor.state.presence {
            let name = short_author(&presence.author);
            let target = Target::Text(id.clone());
            let follow = format!("presence-follow-{id}");
            let view = format!("presence-view-{id}");
            self.bind_command(&view, "agents.view", target.clone());
            self.bind_command(&follow, "agents.follow", target);
            let following = editor.state.follow.as_ref() == Some(id);
            out.push_str(&format!("<panel id='presence-{id}' class='presence-pill'><panel id='presence-dot-{id}' class='presence-dot' /><text id='presence-name-{id}' class='fg' text='{}' /><button id='{view}' class='ghost icon-button' label='View' description='Jump to what {} sees, once' action='Dispatch'>{}</button><button id='{follow}' class='{} icon-button' label='{}' description='{}' action='Dispatch'>{}</button></panel>",escape(&name),escape(&name),icons::icon(&view,"eye"),if following{"selected"}else{"ghost"},if following{"Stop following"}else{"Follow"},escape(&if following{"Stop following".to_owned()}else{format!("Follow {name}'s view")}),if following{icons::icon_in(&follow,"bot","on-selection")}else{icons::icon(&follow,"bot")}));
        }
        out.push_str("</panel>");
        let turns = self.turns(editor);
        if turns.is_empty() {
            self.empty(out, "agents-empty", "bot", "No agent activity yet", "Agents connected over MCP edit with the same commands as you. Their changes apply right away and appear here with the reason, so you can undo a whole turn.", &[("agents-empty-connect", "Connect an agent", "agents.connect")]);
            return;
        }
        out.push_str("<scroll id='agents-scroll' class='body' style='gap:0px'>");
        let now = std::time::Instant::now();
        let names =
            crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap_or_default();
        for turn in &turns {
            let id = format!("turn-{}", turn.key);
            let store = editor.host.store();
            let ops: Vec<&ara_doc::Op> = turn
                .txns
                .iter()
                .filter_map(|t| store.txn(t))
                .flat_map(|t| t.ops.iter())
                .collect();
            let created = ops
                .iter()
                .filter(|o| matches!(o, ara_doc::Op::Create { .. }))
                .count();
            let deleted = ops
                .iter()
                .filter(|o| matches!(o, ara_doc::Op::Delete { .. }))
                .count();
            let changed = ops.len() - created - deleted;
            let when = turn
                .txns
                .first()
                .and_then(|t| editor.activity.seen_at(t))
                .map(|at| ago(now.duration_since(at)))
                // Turns from before this editor session opened.
                .unwrap_or_else(|| "earlier".into());
            // What the turn did, in words: "2 edits · 1 new · 1 removed".
            let counts: Vec<String> = [
                (changed, "edit", "edits"),
                (created, "new", "new"),
                (deleted, "removed", "removed"),
            ]
            .into_iter()
            .filter(|(n, ..)| *n > 0)
            .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
            .collect();
            let expanded = self.expanded_turns.contains(&turn.key);
            let toggle = format!("{id}-toggle");
            self.bind_command(
                &toggle,
                "agents.expand_turn",
                Target::Text(turn.key.clone()),
            );
            out.push_str(&format!("<panel id='{id}' class='feed-row{}'><button id='{toggle}' class='feed-main' label='{}' description='Show what this turn changed' action='Dispatch'>{}<panel id='{id}-avatar' class='avatar-sm'><text id='{id}-initial' class='on-selection' text='{}' /></panel><panel id='{id}-copy' class='feed-copy'><text id='{id}-why' class='feed-why' text='{}' /><text id='{id}-meta' class='caption' text='{} · {when} · {}' /></panel></button>",
                if turn.undone { " feed-undone" } else { "" },
                escape(&turn.why),
                icons::icon_in(&toggle, if expanded { "chevron-down" } else { "chevron-right" }, "icon-sm"),
                escape(&short_author(&turn.author).chars().next().map(String::from).unwrap_or_default()),
                escape(&turn.why),
                escape(&short_author(&turn.author)),
                escape(&counts.join(" · ")),
            ));
            if turn.undone {
                out.push_str(&format!(
                    "<text id='{id}-undone' class='caption' text='Undone' />"
                ));
            } else {
                let undo = format!("{id}-undo");
                self.bind_command(&undo, "agents.undo_turn", Target::Text(turn.key.clone()));
                out.push_str(&format!("<button id='{undo}' class='ghost icon-button row-action' label='Undo turn' description='Undo every change of this turn' action='Dispatch'>{}</button>",icons::icon(&undo,"undo-2")));
            }
            out.push_str("</panel>");
            if expanded {
                out.push_str(&format!("<panel id='{id}-diff' class='feed-diff'>"));
                for (index, op) in ops.iter().enumerate().take(40) {
                    let name = |id: &str| {
                        names
                            .get(id)
                            .and_then(|e| e.record.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .to_owned()
                    };
                    let (class, text) = match op {
                        ara_doc::Op::Set {
                            id,
                            path,
                            prev,
                            value,
                            ..
                        } => (
                            "warning",
                            format!(
                                "{} · {path}   {} → {}",
                                name(id.as_str()),
                                display(prev.as_ref().unwrap_or(&Value::Null)),
                                display(value.as_ref().unwrap_or(&Value::Null))
                            ),
                        ),
                        ara_doc::Op::Create { id, .. } => {
                            ("success", format!("+ {}", name(id.as_str())))
                        }
                        ara_doc::Op::Delete { id, .. } => {
                            ("destructive", format!("− {}", name(id.as_str())))
                        }
                        ara_doc::Op::Move { id, .. } => {
                            ("info", format!("{} moved", name(id.as_str())))
                        }
                        ara_doc::Op::Document { path, kind, .. } => match kind {
                            Some(kind) => ("success", format!("+ {path} ({kind})")),
                            None => ("destructive", format!("− {path}")),
                        },
                    };
                    out.push_str(&format!(
                        "<text id='{id}-op-{index}' class='feed-op {class}' text='{}' />",
                        escape(&text)
                    ));
                }
                if ops.len() > 40 {
                    out.push_str(&format!(
                        "<text id='{id}-more' class='caption' text='{} more changes' />",
                        ops.len() - 40
                    ));
                }
                out.push_str("</panel>");
            }
        }
        out.push_str("</scroll>");
    }

    /// Agent turns newest first: transactions grouped by turn, reverts folded in.
    fn turns(&self, editor: &Editor) -> Vec<Turn> {
        let store = editor.host.store();
        let mut turns: Vec<Turn> = Vec::new();
        for txn in store.history().iter().rev().take(400) {
            if !matches!(txn.author, ara_ops::Author::Agent(_)) || txn.reverts.is_some() {
                continue;
            }
            // Gesture safety may split one turn into several transactions; ara-host labels
            // them `gesture-turn:<group>|<label>`.
            let key = txn
                .label
                .as_deref()
                .and_then(|l| l.strip_prefix("gesture-turn:"))
                .and_then(|l| l.split_once('|'))
                .map(|(group, _)| group.to_owned())
                .unwrap_or_else(|| txn.id.0.clone());
            let undone = store.reverted_by(&txn.id).is_some();
            if let Some(turn) = turns.iter_mut().find(|t| t.key == key) {
                turn.txns.push(txn.id.clone());
                turn.undone &= undone;
                continue;
            }
            if turns.len() >= 100 {
                break;
            }
            turns.push(Turn {
                key,
                author: txn.author.to_string(),
                why: txn
                    .why
                    .clone()
                    .or_else(|| {
                        txn.label.as_deref().map(|l| {
                            l.strip_prefix("gesture-turn:")
                                .and_then(|l| l.split_once('|'))
                                .map_or(l, |(_, label)| label)
                                .to_owned()
                        })
                    })
                    .unwrap_or_else(|| "Edit".into()),
                txns: vec![txn.id.clone()],
                undone,
            });
        }
        turns
    }
}

/// "just now", "12 s ago", "3 min ago", "2 h ago".
fn ago(elapsed: std::time::Duration) -> String {
    let s = elapsed.as_secs();
    match s {
        0..=4 => "just now".into(),
        5..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}
