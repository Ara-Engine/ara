//! Agents: who is attached to each open project and what they are doing (a feed of
//! presence; nothing here asks anyone to approve anything, D24), and the one-time MCP setup.
//! Live activity leads while anyone is attached; otherwise setup leads.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::TextRole;
use stillkit::el::{Align, El, Justify, col, el, icon, row, text};

use super::{HubApp, shell};
use crate::model::{AgentClient, AgentSetup, Intent, Participant, Presence, State, text as fmt};

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
        if connected(s) {
            "Open a project in the editor, or ask a connected agent to open one. Editors, people and agents working in each project appear here with what they are doing."
        } else {
            "Open a project in the editor or headless, then connect an agent. Editors, people and agents working in each project appear here with what they are doing."
        },
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

/// Whether any agent client already has Ara registered.
pub(super) fn connected(s: &HubApp) -> bool {
    s.hub
        .agent_setup
        .values()
        .any(|state| *state == AgentSetup::Connected)
}

/// A client's mark in a `size` px slot.
fn logo(s: &HubApp, client: AgentClient, size: f32) -> El<HubApp> {
    match s.logos.get(&client) {
        Some(image) => stillkit::el::image(image.clone()).size(size),
        None => icon(icons::BOT).size(size),
    }
}

/// One-time global MCP setup. Clients the Hub finds connect in one click; manual steps show
/// only on request. Once one is connected the panel folds to a summary row.
fn connect(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let done: Vec<AgentClient> = AgentClient::ALL
        .into_iter()
        .filter(|client| hub.agent_setup.get(client) == Some(&AgentSetup::Connected))
        .collect();
    if !done.is_empty() && !hub.agent_setup_open {
        let names: Vec<&str> = done.iter().map(|client| client.name()).collect();
        let mut marks = row().gap(4.0);
        for client in &done {
            marks = marks.child(logo(s, *client, 20.0));
        }
        return card(l).pad(14.0).child(
            row()
                .gap(12.0)
                .align(Align::Center)
                .child(marks)
                .child(
                    col()
                        .gap(2.0)
                        .grow(1.0)
                        .child(
                            text(format!("Connected to {}", names.join(", ")))
                                .text_size(12.5)
                                .weight(560)
                                .color(c.foreground.0),
                        )
                        .child(
                            text("Agents see every project registered in this Hub.")
                                .text_size(11.5)
                                .color(c.muted_foreground.0),
                        ),
                )
                .child(
                    button(
                        l,
                        Tone::Secondary,
                        Size::Sm,
                        Some(icons::PLUS),
                        "Connect another agent",
                    )
                    .key("agents-open")
                    .on_press(|s: &mut HubApp| s.intent(Intent::ShowAgentSetup(true))),
                ),
        );
    }
    let mut head = row()
        .gap(14.0)
        .align(Align::Center)
        .child(el().size(40.0).radius(10.0).fill(c.primary.0.with_alpha(0.14)).align(Align::Center).justify(Justify::Center).child(icon(icons::PLUG).size(19.0).color(c.info_foreground.0)))
        .child(
            col()
                .gap(3.0)
                .grow(1.0)
                .child(text("Connect AI agents").font(TextRole::Title3).color(c.foreground.0))
                .child(text("One global MCP server for every project. Agents list, create, open and edit projects registered in this Hub.").text_size(12.0).wrap(2).color(c.muted_foreground.0)),
        );
    if !done.is_empty() {
        head = head.child(
            icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Done")
                .key("agents-close")
                .on_press(|s: &mut HubApp| s.intent(Intent::ShowAgentSetup(false))),
        );
    }
    let mut clients = col().gap(2.0);
    for client in AgentClient::ALL {
        clients = clients.child(client_row(s, client));
    }
    let mut panel = card(l).pad(18.0).gap(14.0).child(head).child(clients);
    if !done.is_empty() {
        panel = panel.child(
            text("Restart a newly connected agent, then ask it to list your projects.")
                .text_size(11.5)
                .color(c.muted_foreground.0),
        );
    }
    panel
}

/// One client: its mark, what the Hub found, and the one action that fits.
fn client_row(s: &HubApp, client: AgentClient) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let state = s.hub.agent_setup.get(&client);
    let manual = s.hub.agent_manual == Some(client);
    let status = match state {
        None => "Looking for it…",
        Some(AgentSetup::Missing) => "Not found on this computer",
        Some(AgentSetup::Available) => "Installed · not connected yet",
        Some(AgentSetup::Working) => "Connecting…",
        Some(AgentSetup::Connected) => "Ara is connected",
        Some(AgentSetup::Failed(_)) => "Couldn't connect",
    };
    let id = client.id();
    let action: El<HubApp> = match state {
        Some(AgentSetup::Available) => {
            button(l, Tone::Primary, Size::Sm, Some(icons::PLUG), "Connect")
                .key(format!("agent-{id}-connect"))
                .on_press(move |s: &mut HubApp| s.intent(Intent::SetupAgent(client)))
        }
        Some(AgentSetup::Failed(_)) => button(
            l,
            Tone::Secondary,
            Size::Sm,
            Some(icons::REFRESH_CW),
            "Retry",
        )
        .key(format!("agent-{id}-retry"))
        .on_press(move |s: &mut HubApp| s.intent(Intent::SetupAgent(client))),
        Some(AgentSetup::Connected) => chip(l, Status::Success, "Connected", true),
        Some(AgentSetup::Working) | None => spinner(l, 14.0),
        Some(AgentSetup::Missing) => el(),
    };
    let mut line = row()
        .key(format!("agent-{id}"))
        .min_h(48.0)
        .pad_x(10.0)
        .pad_y(6.0)
        .gap(12.0)
        .align(Align::Center)
        .child(logo(s, client, 22.0))
        .child(
            col()
                .gap(2.0)
                .grow(1.0)
                .child(
                    text(client.name())
                        .text_size(12.5)
                        .weight(560)
                        .color(c.foreground.0),
                )
                .child(text(status).text_size(11.5).color(c.muted_foreground.0)),
        )
        .child(action);
    // Manual steps only where the Hub cannot connect the client itself.
    if matches!(state, Some(AgentSetup::Missing | AgentSetup::Failed(_))) {
        let (label, intent) = if manual {
            ("Hide steps", Intent::ManualAgent(None))
        } else {
            ("Set up manually", Intent::ManualAgent(Some(client)))
        };
        line = line.child(
            button(l, Tone::Ghost, Size::Sm, None, label)
                .key(format!("agent-{id}-manual"))
                .on_press(move |s: &mut HubApp| s.intent(intent.clone())),
        );
    }
    let mut out = col().gap(6.0).child(line);
    if let Some(AgentSetup::Failed(error)) = state {
        out = out.child(
            text(fmt::clip(error, 240))
                .text_size(11.5)
                .wrap(3)
                .color(c.muted_foreground.0)
                .pad_x(10.0),
        );
    }
    if manual {
        out = out.child(
            col()
                .pad_x(10.0)
                .pad_y(6.0)
                .gap(14.0)
                .children(manual_steps(s, client)),
        );
    }
    out
}

/// The copyable steps for a client the Hub could not find or connect.
fn manual_steps(s: &HubApp, client: AgentClient) -> Vec<El<HubApp>> {
    let c = &s.look.c;
    let Some(executable) = s.hub.cli_executable.as_ref() else {
        return vec![
            text("Preparing Ara connection instructions…")
                .text_size(12.0)
                .color(c.muted_foreground.0),
        ];
    };
    let path = executable.to_string_lossy();
    let quoted = if cfg!(windows) {
        format!("'{}'", path.replace('\'', "''"))
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    let invoke = if cfg!(windows) { "& " } else { "" };
    let install = format!("{invoke}{quoted} hub install-mcp --client {}", client.id());
    let printed = s
        .hub
        .install
        .get(&client)
        .map(String::as_str)
        .filter(|o| !o.trim().is_empty());
    let id = format!("copy-{}", client.id());
    let mut steps = Vec::new();
    if client == AgentClient::Claude {
        steps.push((
            "Register Ara with Claude Code (user scope)".to_owned(),
            shell::code(
                s,
                &format!("{id}-add"),
                &format!("claude mcp add --scope user ara -- {quoted} mcp"),
            ),
        ));
    }
    let config = match client {
        AgentClient::Claude => "Or add this entry to ~/.claude.json",
        AgentClient::Codex => "Add this to ~/.codex/config.toml",
        AgentClient::Cursor => "Add this server to ~/.cursor/mcp.json",
        AgentClient::OpenCode => "Add this to ~/.config/opencode/opencode.json",
    };
    steps.push((
        config.to_owned(),
        shell::code(
            s,
            &format!("{id}-config"),
            printed.map_or(install.as_str(), str::trim_end),
        ),
    ));
    steps.push((
        "Restart the agent and ask it to list your projects".to_owned(),
        text("Agents call ara_projects to list, create, open headless and close projects. Their edits apply immediately and stay undoable per turn.").text_size(12.0).wrap(4).color(c.muted_foreground.0),
    ));
    steps
        .into_iter()
        .enumerate()
        .map(|(n, (title, content))| step(s, n + 1, &title, content))
        .collect()
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
