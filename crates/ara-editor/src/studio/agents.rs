//! Agents and History: who changed the project, when and why.
//!
//! **Agents** (D24, amended): agent edits apply at once, so this is a feed, not a review
//! queue. Connected agents sit on top with View (jump once to what they see) and Follow;
//! below, each agent turn shows its reason, its author, when, what it did in counts, and
//! Undo Turn; expanding a turn lists its ops. **History** is every transaction, human and
//! agent, newest first, with Undo and Redo: one gesture is one row.
use std::time::Instant;

use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::Rgba;
use stillkit::el::{Align, Axis, El, col, el, icon, row, text};

use super::Studio;
use crate::activity::short_author;
use crate::shell::agents_panel::{ago, file_change, turns};

/// The Agents panel.
pub(super) fn agents(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let mut page = col().gap(6.0).pad(8.0);
    // Who is connected, and what they see.
    let presence = &s.editor.state.presence;
    if presence.is_empty() {
        page = page.child(connect_card(s));
    } else {
        for (id, p) in presence {
            page = page.child(presence_row(s, id, p));
        }
    }
    let store = s.editor.host.store();
    let all = turns(store);
    if all.is_empty() {
        page = page.child(
            row()
                .gap(6.0)
                .align(Align::Center)
                .pad_x(2.0)
                .child(icon(icons::CLOCK).size(12.0).color(c.muted_foreground.0))
                .child(
                    text(
                        "Agent turns show here with their reasons; each can be undone as a whole.",
                    )
                    .text_size(11.5)
                    .color(c.muted_foreground.0),
                ),
        );
        return col()
            .grow(1.0)
            .min_h(0.0)
            .scroll(Axis::Vertical)
            .child(page);
    }
    page = page.child(
        row()
            .h(20.0)
            .align(Align::Center)
            .child(
                text("Recent turns")
                    .text_size(11.0)
                    .weight(600)
                    .color(c.muted_foreground.0),
            )
            .child(el().grow(1.0))
            .child(
                text(format!("{}", all.len()))
                    .text_size(11.0)
                    .color(c.muted_foreground.0),
            ),
    );
    let now = Instant::now();
    for turn in &all {
        page = page.child(turn_card(s, turn, now));
    }
    col()
        .grow(1.0)
        .min_h(0.0)
        .scroll(Axis::Vertical)
        .child(page)
}

/// How to connect an agent, with the command to run.
fn connect_card(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let command = format!("ara mcp --root \"{}\"", s.editor.host.root().display());
    col().radius(super::style::RADIUS).fill(super::style::tones(l).band)
        .pad(10.0)
        .gap(6.0)
        .child(row().gap(8.0).align(Align::Center).child(icon(icons::BOT).size(15.0).color(c.primary.0)).child(text("No agent connected").text_size(12.5).weight(600).color(c.foreground.0)))
        .child(text("Claude Code, Codex and other MCP clients use the same commands as this editor. Run this in the project, or add it to the client's MCP config:").text_size(11.5).wrap(3).color(c.muted_foreground.0))
        .child(row().pad_x(8.0).pad_y(6.0).radius(l.radius.sm).fill(c.code_background.0).child(text(command).text_size(11.0).family(mono()).color(c.code_foreground.0)))
}

/// A connected agent: a pulse, its name, what it works on, View and Follow.
fn presence_row(s: &Studio, id: &str, p: &crate::state::AgentPresence) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let name = short_author(&p.author);
    let following = s.editor.state.follow.as_deref() == Some(id);
    let (view, follow) = (id.to_owned(), id.to_owned());
    let doing = match p.selection.len() {
        0 => "in the scene".to_owned(),
        1 => format!("on {}", s.name(&p.selection[0])),
        n => format!("on {n} entities"),
    };
    row()
        .key(format!("agents/presence/{id}"))
        .h(36.0)
        .pad_x(8.0)
        .gap(8.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .fill(c.success.0.with_alpha(0.08))
        .border(1.0, c.success.0.with_alpha(0.25))
        .child(dot(l, Status::Success))
        .child(avatar(l, &name, ara_kit::art::seed(&p.author), 20.0))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(text(name).text_size(12.5).weight(600).color(c.foreground.0))
                .child(text(doing).text_size(11.0).color(c.muted_foreground.0)),
        )
        .child(
            button(l, Tone::Ghost, Size::Sm, Some(icons::EYE), "View").on_press(
                move |s: &mut Studio| {
                    s.act("editor.follow", json!({"agent": view}));
                    s.act("editor.follow", json!({}));
                },
            ),
        )
        .child(
            toggle_button(
                l,
                Size::Sm,
                icons::BOT,
                if following {
                    "Stop following"
                } else {
                    "Follow"
                },
                following,
            )
            .on_press(move |s: &mut Studio| {
                if s.editor.state.follow.as_deref() == Some(follow.as_str()) {
                    s.act("editor.follow", json!({}));
                } else {
                    s.act("editor.follow", json!({"agent": follow}));
                }
            }),
        )
}

/// One agent turn: why, who, when, what it did; Undo Turn; its ops when open.
fn turn_card(s: &Studio, turn: &crate::shell::agents_panel::Turn, now: Instant) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let store = s.editor.host.store();
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
    let counts: Vec<String> = [
        (changed, "edit", "edits"),
        (created, "new", "new"),
        (deleted, "removed", "removed"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
    .collect();
    let when = turn
        .txns
        .first()
        .and_then(|t| s.editor.activity.seen_at(t))
        .map(|at| ago(now.duration_since(at)))
        .unwrap_or_else(|| "earlier".into());
    let name = short_author(&turn.author);
    let open = s.local.expanded_turns.contains(&turn.key);
    let key = turn.key.clone();
    let meta = format!(
        "{name} · {when}{}",
        if counts.is_empty() {
            String::new()
        } else {
            format!(" · {}", counts.join(" · "))
        }
    );
    let head = row()
        .key(format!("agents/turn/{}", turn.key))
        .min_h(40.0)
        .pad_x(8.0)
        .pad_y(6.0)
        .gap(10.0)
        .align(Align::Center)
        .label(turn.why.clone())
        .expanded(open)
        .on_press(move |s: &mut Studio| {
            if !s.local.expanded_turns.remove(&key) {
                s.local.expanded_turns.insert(key.clone());
            }
        })
        .child(avatar(l, &name, ara_kit::art::seed(&turn.author), 22.0))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(turn.why.clone())
                        .text_size(12.5)
                        .weight(560)
                        .wrap(2)
                        .color(if turn.undone {
                            c.muted_foreground.0
                        } else {
                            c.foreground.0
                        }),
                )
                .child(text(meta).text_size(11.0).color(c.muted_foreground.0)),
        );
    let head = if turn.undone {
        head.child(chip(l, Status::Neutral, "Undone", false))
    } else {
        let txns = turn.txns.clone();
        head.child(
            icon_button(l, Tone::Ghost, Size::Sm, icons::UNDO_2, "Undo turn").on_press(
                move |s: &mut Studio| {
                    // Newest first, so each revert applies on top of what came after it.
                    for txn in &txns {
                        if s.editor.host.store().reverted_by(txn).is_none() {
                            s.act("review.revert", json!({"txn": txn.0}));
                        }
                    }
                    s.resolve();
                },
            ),
        )
    };
    let mut out = col()
        .radius(super::style::RADIUS)
        .fill(super::style::tones(l).band)
        .child(head);
    if open {
        let mut diff = col().pad_x(10.0).pad_y(6.0).gap(2.0);
        for op in ops.iter().take(40) {
            let (tone, line) = op_line(s, op);
            diff = diff.child(
                row()
                    .gap(6.0)
                    .align(Align::Center)
                    .child(el().w(3.0).h(14.0).radius(1.5).fill(tone))
                    .child(
                        text(line)
                            .text_size(11.5)
                            .family(mono())
                            .color(c.foreground.0),
                    ),
            );
        }
        if ops.len() > 40 {
            diff = diff.child(
                text(format!("and {} more", ops.len() - 40))
                    .text_size(11.0)
                    .color(c.muted_foreground.0),
            );
        }
        out = out.child(diff);
    }
    out
}

/// An op in words, with its tone: green created, red removed, amber changed.
fn op_line(s: &Studio, op: &ara_doc::Op) -> (Rgba, String) {
    let c = &s.look.c;
    let short = |v: Option<&Value>| {
        let t = v.map_or("—".to_owned(), Value::to_string);
        if t.chars().count() > 28 {
            format!("{}…", t.chars().take(27).collect::<String>())
        } else {
            t
        }
    };
    match op {
        ara_doc::Op::Set {
            id,
            path,
            prev,
            value,
            ..
        } => (
            c.warning.0,
            format!(
                "{} · {path}  {} → {}",
                s.name(id.as_str()),
                short(prev.as_ref()),
                short(value.as_ref())
            ),
        ),
        ara_doc::Op::Create { id, .. } => (c.success.0, format!("+ {}", s.name(id.as_str()))),
        ara_doc::Op::Delete { id, .. } => (c.destructive.0, format!("− {}", s.name(id.as_str()))),
        ara_doc::Op::File {
            path, prev, state, ..
        } => {
            let (tone, line) = file_change(path, prev.as_ref(), state.as_ref());
            (
                if tone == "success" {
                    c.success.0
                } else if tone == "destructive" {
                    c.destructive.0
                } else {
                    c.warning.0
                },
                line,
            )
        }
        other => (c.info.0, format!("{other:?}").chars().take(60).collect()),
    }
}

/// The History panel: every transaction, newest first, with Undo and Redo.
pub(super) fn history(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let store = s.editor.host.store();
    let toolbar = row()
        .h(30.0)
        .pad_x(6.0)
        .gap(4.0)
        .align(Align::Center)
        .child(
            button(l, Tone::Ghost, Size::Sm, Some(icons::UNDO_2), "Undo").on_press(
                |s: &mut Studio| {
                    super::commands::run(s, "edit.undo", &crate::catalog::Target::None)
                },
            ),
        )
        .child(
            button(l, Tone::Ghost, Size::Sm, Some(icons::REDO_2), "Redo").on_press(
                |s: &mut Studio| {
                    super::commands::run(s, "edit.redo", &crate::catalog::Target::None)
                },
            ),
        )
        .child(el().grow(1.0))
        .child(
            text(match store.history().len() {
                1 => "1 change".to_owned(),
                n => format!("{n} changes"),
            })
            .text_size(11.0)
            .color(c.muted_foreground.0),
        );
    let history = store.history();
    if history.is_empty() {
        return col().grow(1.0).child(toolbar).child(empty(
            l,
            icons::CLOCK,
            "No changes yet",
            "Every edit, yours or an agent's, lands here as one step you can undo.",
            None,
        ));
    }
    let now = Instant::now();
    let mut list = col().pad(4.0).gap(1.0);
    for txn in history.iter().rev().take(300) {
        let undone = store.reverted_by(&txn.id).is_some();
        let revert = txn.reverts.is_some();
        let who = match &txn.author {
            ara_ops::Author::Human => "You".to_owned(),
            other => short_author(&other.to_string()),
        };
        let what = txn
            .why
            .clone()
            .or_else(|| txn.label.clone())
            .unwrap_or_else(|| "Edit".into());
        let when = s
            .editor
            .activity
            .seen_at(&txn.id)
            .map(|at| ago(now.duration_since(at)))
            .unwrap_or_else(|| "earlier".into());
        let glyph = if revert {
            icons::UNDO_2
        } else if matches!(txn.author, ara_ops::Author::Human) {
            icons::USER
        } else {
            icons::BOT
        };
        let ink = if undone {
            c.muted_foreground.0
        } else {
            c.foreground.0
        };
        list = list.child(
            row()
                .h(28.0)
                .pad_x(8.0)
                .gap(8.0)
                .align(Align::Center)
                .radius(4.0)
                .child(icon(glyph).size(13.0).color(c.muted_foreground.0))
                .child(
                    text(if revert {
                        format!("Undo: {what}")
                    } else {
                        what
                    })
                    .text_size(12.0)
                    .color(ink)
                    .grow(1.0),
                )
                .child(
                    text(if undone { "undone".to_owned() } else { who })
                        .text_size(11.0)
                        .color(c.muted_foreground.0),
                )
                .child(
                    text(when)
                        .text_size(11.0)
                        .color(c.muted_foreground.0.with_alpha(0.7))
                        .w(70.0)
                        .text_align(stillkit::el::TextAlign::End),
                ),
        );
    }
    col()
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar)
        .child(col().h(0.0).grow(1.0).scroll(Axis::Vertical).child(list))
}
