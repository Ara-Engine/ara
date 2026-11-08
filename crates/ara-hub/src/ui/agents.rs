//! Agents: who is attached to each open project and what they are doing (a feed of
//! presence; nothing here asks anyone to approve anything, D24), and the one-time MCP setup.
//! Live activity leads while anyone is attached; otherwise setup leads.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::TextRole;
use stillkit::el::{Align, El, Justify, col, el, icon, row, text};

use super::{HubApp, shell};
use crate::model::{AgentClient, Intent, Participant, Presence, State, text as fmt};

pub(super) fn body(s: &HubApp) -> El<HubApp> {
    let live: Vec<_> = s
        .hub
        .activity
        .iter()
        .filter(|a| !a.participants.is_empty())
        .collect();
    if live.is_empty() {
        return col().gap(24.0).child(connect(s)).child(activity_empty(s));
    }
    let l = &s.look;
    let mut feed = col()
        .gap(12.0)
        .child(section(l, "Live activity", None, Vec::new()));
    for activity in live {
        let Some(project) = s.hub.project(&activity.project) else {
            continue;
        };
        let state = match s.hub.state(&project.id) {
            State::Editor => chip(l, Status::Success, "Open in editor", true),
            State::Headless => chip(l, Status::Accent, "Running headless", true),
            State::Closed => chip(l, Status::Neutral, "Closed", false),
        };
        let mut rows = card(l).child(
            row()
                .h(44.0)
                .pad_x(14.0)
                .gap(10.0)
                .align(Align::Center)
                .child(avatar(
                    l,
                    &project.name,
                    ara_kit::art::seed(&project.id),
                    24.0,
                ))
                .child(
                    text(project.name.clone())
                        .font(TextRole::Headline)
                        .color(l.c.foreground.0)
                        .grow(1.0),
                )
                .child(state),
        );
        for person in &activity.participants {
            rows = rows.child(divider(l)).child(participant(
                s,
                person,
                activity.presence.get(&person.id),
            ));
        }
        feed = feed.child(rows);
    }
    col().gap(24.0).child(feed).child(connect(s))
}

fn activity_empty(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    col().gap(12.0).child(section(l, "Live activity", None, Vec::new())).child(card(l).child(empty(
        l,
        icons::USERS,
        "No one is attached right now",
        "Open a project in the editor or headless, then connect an agent. People and agents working in each project appear here with what they are doing.",
        None,
    )))
}

fn participant(s: &HubApp, person: &Participant, presence: Option<&Presence>) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let agent = person.kind == "agent";
    let kind = if agent {
        "Agent"
    } else if person.name.to_lowercase().contains("editor") {
        "Editor"
    } else {
        "Person"
    };
    let mut doing = Vec::new();
    if let Some(p) = presence {
        if let Some(task) = &p.activity {
            doing.push(match &task.why {
                Some(why) => format!("{} — {why}", task.what),
                None => task.what.clone(),
            });
        }
        if let Some(doc) = &p.doc {
            doing.push(format!("in {doc}"));
        }
        match p.selection.len() {
            0 => {}
            1 => doing.push(format!("selected {}", p.selection[0])),
            n => doing.push(format!("{n} selected")),
        }
    }
    let busy = !doing.is_empty();
    let line = if busy {
        doing.join(" · ")
    } else {
        "Idle".to_owned()
    };
    let tint = if agent {
        c.info_foreground.0
    } else {
        c.success_foreground.0
    };
    row()
        .h(52.0)
        .pad_x(14.0)
        .gap(12.0)
        .align(Align::Center)
        .child(
            el().size(30.0)
                .radius(15.0)
                .fill(tint.with_alpha(0.14))
                .align(Align::Center)
                .justify(Justify::Center)
                .child(
                    icon(if agent { icons::BOT } else { icons::USER })
                        .size(15.0)
                        .color(tint),
                ),
        )
        .child(
            col()
                .gap(2.0)
                .grow(1.0)
                .min_w(0.0)
                .child(
                    row()
                        .gap(8.0)
                        .align(Align::Center)
                        .child(
                            text(person.name.clone())
                                .text_size(12.5)
                                .weight(560)
                                .color(c.foreground.0),
                        )
                        .child(chip(l, Status::Neutral, kind, false)),
                )
                .child(
                    text(fmt::clip(&line, 140))
                        .text_size(11.5)
                        .color(c.muted_foreground.0),
                ),
        )
        .child(if busy { spinner(l, 12.0) } else { el() })
}

fn connect(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let client = hub.prefs.agent;
    let picker = segmented(
        l,
        &[
            ("agent-claude", None, "Claude Code"),
            ("agent-codex", None, "Codex"),
        ],
        if client == AgentClient::Claude { 0 } else { 1 },
        |i| {
            move |s: &mut HubApp| {
                s.intent(Intent::Agent(if i == 0 {
                    AgentClient::Claude
                } else {
                    AgentClient::Codex
                }))
            }
        },
    );
    let head = row().gap(14.0).align(Align::Center)
        .child(el().size(40.0).radius(10.0).fill(c.primary.0.with_alpha(0.14)).align(Align::Center).justify(Justify::Center).child(icon(icons::PLUG).size(19.0).color(c.info_foreground.0)))
        .child(col().gap(3.0).grow(1.0).child(text("Connect an AI agent").font(TextRole::Title3).color(c.foreground.0)).child(text("One global MCP server for every project. Install it once per agent client.").text_size(12.0).color(c.muted_foreground.0)))
        .child(picker);
    let mut panel = card(l).pad(18.0).gap(16.0).child(head).child(divider(l));
    let Some(executable) = hub.cli_executable.as_ref() else {
        return panel.child(
            row()
                .gap(8.0)
                .align(Align::Center)
                .child(spinner(l, 14.0))
                .child(
                    text("Preparing connection instructions…")
                        .text_size(12.0)
                        .color(c.muted_foreground.0),
                ),
        );
    };
    let path = executable.to_string_lossy();
    let quoted = if cfg!(windows) {
        format!("'{}'", path.replace('\'', "''"))
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    let invoke = if cfg!(windows) { "& " } else { "" };
    let printed = hub
        .install
        .get(&client)
        .map(String::as_str)
        .filter(|o| !o.trim().is_empty());
    let install = format!("{invoke}{quoted} hub install-mcp --client {}", client.id());
    let block = |key: &str| shell::code(s, key, printed.map_or(install.as_str(), str::trim_end));
    let steps: Vec<(&str, El<HubApp>)> = match client {
        AgentClient::Claude => vec![
            (
                "Register Ara with Claude Code (user scope)",
                shell::code(
                    s,
                    "copy-claude-add",
                    &format!("claude mcp add --scope user ara -- {quoted} mcp"),
                ),
            ),
            (
                "Or add this entry to ~/.claude.json yourself",
                block("copy-claude-json"),
            ),
        ],
        AgentClient::Codex => vec![
            ("Add this to ~/.codex/config.toml", block("copy-codex-toml")),
            (
                "Print it again any time",
                shell::code(s, "copy-codex-install", &install),
            ),
        ],
    };
    let mut n = 0;
    for (title, content) in steps {
        n += 1;
        panel = panel.child(step(s, n, title, content));
    }
    panel.child(step(
        s,
        n + 1,
        "Restart the agent and ask it to list your projects",
        text("Agents call ara_projects to list, create, open headless and close projects. They only see projects registered in this Hub; their edits apply immediately and stay undoable per turn.").text_size(12.0).wrap(4).color(c.muted_foreground.0),
    ))
}

fn step(s: &HubApp, n: usize, title: &str, content: El<HubApp>) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    row()
        .gap(14.0)
        .align(Align::Start)
        .child(
            el().size(24.0)
                .radius(12.0)
                .fill(c.raised.0)
                .border(1.0, c.border.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .child(
                    text(n.to_string())
                        .text_size(11.5)
                        .weight(650)
                        .color(c.foreground.0),
                ),
        )
        .child(
            col()
                .gap(8.0)
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(title.to_owned())
                        .text_size(12.5)
                        .weight(560)
                        .color(c.foreground.0)
                        .pad_y(3.0),
                )
                .child(content),
        )
}
