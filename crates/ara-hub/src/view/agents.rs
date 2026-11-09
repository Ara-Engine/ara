//! Agents: one-time global MCP setup per client (detected clients connect in one
//! click; manual steps only on request), then who is attached to each project.
//! Activity is a feed of presence; nothing here asks anyone to approve anything (D24).
use super::Page;
use crate::model::{AgentClient, AgentSetup, Hub, Intent, Participant, Presence, State, text};

pub(super) fn body(p: &mut Page, hub: &Hub) {
    let live: Vec<_> = hub
        .activity
        .iter()
        .filter(|a| !a.participants.is_empty())
        .collect();
    if live.is_empty() {
        connect(p, hub);
    }
    p.text("section-title", "Live activity");
    if live.is_empty() {
        p.panel("empty", |p| {
            p.panel("empty-icon", |p| p.icon("users", "icon-lg"));
            p.text("empty-title", "No one is attached right now");
            p.text(
                "empty-copy",
                if connected(hub) {
                    "Open a project in the editor, or ask a connected agent to open one. Editors, people and agents working in each project appear here with what they are doing."
                } else {
                    "Open a project in the editor or headless, then connect an agent. Editors, people and agents working in each project appear here with what they are doing."
                },
            );
        });
        return;
    }
    for activity in live {
        let Some(project) = hub.project(&activity.project) else {
            continue;
        };
        p.panel("list", |p| {
            p.panel("list-head", |p| {
                p.icon("box", "icon-sm");
                p.text("fg fill", &project.name);
                match hub.state(&project.id) {
                    State::Editor => p.panel("pill pill-open", |p| {
                        p.leaf("pill-dot");
                        p.text("", "Open in editor");
                    }),
                    State::Headless => p.panel("pill pill-headless", |p| {
                        p.leaf("pill-dot pill-dot-headless");
                        p.text("", "Running headless");
                    }),
                    State::Closed => p.text("", "Closed"),
                }
            });
            for (row, person) in activity.participants.iter().enumerate() {
                if row > 0 {
                    p.leaf("divider");
                }
                let presence = activity.presence.get(&person.id);
                participant(p, person, presence);
            }
        });
    }
    connect(p, hub);
}

fn participant(p: &mut Page, person: &Participant, presence: Option<&Presence>) {
    let agent = person.kind == "agent";
    p.panel("person", |p| {
        match client_named(&person.name).filter(|_| agent) {
            Some(client) => logo(p, client, "agent-logo"),
            None if agent => p.panel("avatar avatar-agent", |p| p.icon("bot", "inherit")),
            None => p.panel("avatar avatar-human", |p| p.icon("user", "inherit")),
        }
        p.panel("col fill", |p| {
            p.panel("row", |p| {
                p.text("fg", &person.name);
                let kind = if agent {
                    "Agent"
                } else if person.name.to_lowercase().contains("editor") {
                    "Editor"
                } else {
                    "Person"
                };
                p.panel("pill", |p| p.text("", kind));
            });
            let mut doing = Vec::new();
            if let Some(presence) = presence {
                if let Some(task) = &presence.activity {
                    doing.push(match &task.why {
                        Some(why) => format!("{} — {why}", task.what),
                        None => task.what.clone(),
                    });
                }
                if let Some(doc) = &presence.doc {
                    doing.push(format!("in {doc}"));
                }
                match presence.selection.len() {
                    0 => {}
                    1 => doing.push(format!("selected {}", presence.selection[0])),
                    n => doing.push(format!("{n} selected")),
                }
            }
            let line = if doing.is_empty() {
                "Idle".to_owned()
            } else {
                doing.join(" · ")
            };
            p.text("small muted", &text::clip(&line, 110));
        });
    });
}

/// Whether any agent client already has Ara registered.
pub(super) fn connected(hub: &Hub) -> bool {
    hub.agent_setup
        .values()
        .any(|s| *s == AgentSetup::Connected)
}

/// The client an attached agent most likely is, from its reported name.
fn client_named(name: &str) -> Option<AgentClient> {
    let name = name.to_lowercase();
    AgentClient::ALL
        .into_iter()
        .find(|client| name.contains(client.id()))
}

/// A client's mark: Claude in its brand colour, the others tinted like icons.
fn logo(p: &mut Page, client: AgentClient, class: &str) {
    p.panel(class, |p| match client {
        AgentClient::Claude => p.image("brand-claude", "agent-mark"),
        other => p.icon(&format!("agent-{}", other.id()), "agent-mark"),
    });
}

/// Once an agent is connected the setup collapses to one summary row; the full
/// panel (other clients, manual steps) stays one click away.
fn connect(p: &mut Page, hub: &Hub) {
    let done: Vec<_> = AgentClient::ALL
        .into_iter()
        .filter(|client| hub.agent_setup.get(client) == Some(&AgentSetup::Connected))
        .collect();
    if !done.is_empty() && !hub.agent_setup_open {
        p.panel("panel-card", |p| {
            p.panel("row", |p| {
                p.panel("agent-stack", |p| {
                    for client in &done {
                        logo(p, *client, "agent-logo");
                    }
                });
                p.panel("col fill", |p| {
                    let names: Vec<_> = done.iter().map(|client| client.name()).collect();
                    p.text("fg", &format!("Connected to {}", names.join(", ")));
                    p.text(
                        "small muted",
                        "Agents see every project registered in this Hub.",
                    );
                });
                p.action(
                    "agents-open",
                    "compact",
                    "plus",
                    "Connect another agent",
                    Some(Intent::ShowAgentSetup(true)),
                );
            });
        });
        return;
    }
    p.panel("panel-card", |p| {
        p.panel("row", |p| {
            p.panel("engine-icon", |p| p.icon("plug", "icon-lg accent"));
            p.panel("col fill", |p| {
                p.text("engine-version", "Connect AI agents");
                p.text(
                    "small muted",
                    "One global MCP server for every project. Agents list, create, open and edit projects registered in this Hub.",
                );
            });
            if !done.is_empty() {
                p.button(
                    "agents-close",
                    "ghost icon-compact",
                    "Done",
                    Some(Intent::ShowAgentSetup(false)),
                    |p| p.icon("x", "inherit"),
                );
            }
        });
        p.panel("agent-clients", |p| {
            for client in AgentClient::ALL {
                client_row(p, hub, client);
            }
        });
        if !done.is_empty() {
            p.text(
                "small muted",
                "Restart a newly connected agent, then ask it to list your projects.",
            );
        }
    });
}

/// One client: its mark, what the Hub found, and the one action that fits.
fn client_row(p: &mut Page, hub: &Hub, client: AgentClient) {
    let state = hub.agent_setup.get(&client);
    let manual = hub.agent_manual == Some(client);
    p.panel_key(client.id(), "agent-client", |p| {
        p.panel("row", |p| {
            logo(p, client, "agent-logo");
            p.panel("col fill", |p| {
                p.text("fg", client.name());
                let status = match state {
                    None => "Looking for it…",
                    Some(AgentSetup::Missing) => "Not found on this computer",
                    Some(AgentSetup::Available) => "Installed · not connected yet",
                    Some(AgentSetup::Working) => "Connecting…",
                    Some(AgentSetup::Connected) => "Ara is connected",
                    Some(AgentSetup::Failed(_)) => "Couldn't connect",
                };
                p.text("small muted", status);
            });
            let id = format!("agent-{}", client.id());
            match state {
                Some(AgentSetup::Available) => p.action(
                    &format!("{id}-connect"),
                    "primary compact",
                    "plug",
                    "Connect",
                    Some(Intent::SetupAgent(client)),
                ),
                Some(AgentSetup::Failed(_)) => p.action(
                    &format!("{id}-retry"),
                    "compact",
                    "refresh-cw",
                    "Retry",
                    Some(Intent::SetupAgent(client)),
                ),
                Some(AgentSetup::Connected) => p.panel("pill pill-open", |p| {
                    p.leaf("pill-dot");
                    p.text("", "Connected");
                }),
                Some(AgentSetup::Working) | None => p.icon("loader-circle", "icon-sm"),
                Some(AgentSetup::Missing) => {}
            }
            // Manual steps only where the Hub cannot connect the client itself.
            if matches!(state, Some(AgentSetup::Missing | AgentSetup::Failed(_))) {
                let (label, intent) = if manual {
                    ("Hide steps", Intent::ManualAgent(None))
                } else {
                    ("Set up manually", Intent::ManualAgent(Some(client)))
                };
                p.action(&format!("{id}-manual"), "compact", "", label, Some(intent));
            }
        });
        if let Some(AgentSetup::Failed(error)) = state {
            p.text("small muted wrap", &text::clip(error, 240));
        }
        if manual {
            p.panel("agent-manual", |p| manual_steps(p, hub, client));
        }
    });
}

/// The copyable steps for a client the Hub could not find or connect.
fn manual_steps(p: &mut Page, hub: &Hub, client: AgentClient) {
    let copied = hub.copied.as_deref();
    let Some(executable) = hub.cli_executable.as_ref() else {
        p.text("section-copy", "Preparing Ara connection instructions…");
        return;
    };
    let path = executable.to_string_lossy();
    let quoted = if cfg!(windows) {
        format!("'{}'", path.replace('\'', "''"))
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    let invoke = if cfg!(windows) { "& " } else { "" };
    let printed = hub.install.get(&client).map(String::as_str);
    let install = format!("{invoke}{quoted} hub install-mcp --client {}", client.id());
    let id = format!("copy-{}", client.id());
    let config = match client {
        AgentClient::Claude => {
            step(p, "1", "Register Ara with Claude Code (user scope)", |p| {
                code(
                    p,
                    copied,
                    &format!("{id}-add"),
                    &format!("claude mcp add --scope user ara -- {quoted} mcp"),
                );
            });
            "Or add this entry to ~/.claude.json"
        }
        AgentClient::Codex => "Add this to ~/.codex/config.toml",
        AgentClient::Cursor => "Add this server to ~/.cursor/mcp.json",
        AgentClient::OpenCode => "Add this to ~/.config/opencode/opencode.json",
    };
    let [config_step, restart_step] = if client == AgentClient::Claude {
        ["2", "3"]
    } else {
        ["1", "2"]
    };
    step(p, config_step, config, |p| {
        block(p, copied, &format!("{id}-config"), printed, &install);
    });
    step(
        p,
        restart_step,
        "Restart the agent and ask it to list your projects",
        |p| {
            p.text(
                "section-copy",
                "Agents call ara_projects to list, create, open headless and close projects. Their edits apply immediately and stay undoable per turn.",
            );
        },
    );
}

fn step(p: &mut Page, number: &str, title: &str, body: impl FnOnce(&mut Page)) {
    p.panel("step", |p| {
        p.panel("step-num", |p| p.text("", number));
        p.panel("col fill", |p| {
            p.text("fg", title);
            body(p);
        });
    });
}

fn code(p: &mut Page, copied: Option<&str>, id: &str, command: &str) {
    p.panel("code", |p| {
        p.panel("col fill", |p| code_line(p, command));
        p.copy(copied, id, command, false);
    });
}

/// Multi-line output (`install-mcp`), or the command that prints it.
fn block(p: &mut Page, copied: Option<&str>, id: &str, output: Option<&str>, command: &str) {
    let Some(output) = output.filter(|o| !o.trim().is_empty()) else {
        code(p, copied, id, command);
        return;
    };
    p.panel("code code-top", |p| {
        p.panel("col fill", |p| {
            for line in output.lines() {
                code_line(p, line);
            }
        });
        p.copy(copied, id, output.trim_end(), false);
    });
}

/// One shell, TOML or JSON line as tokens coloured by role. Text runs carry one
/// style each, so a line is a wrapping row of runs.
fn code_line(p: &mut Page, line: &str) {
    let trimmed = line.trim_start();
    let indent = (line.len() - trimmed.len()) as f32 * 7.2;
    p.panel_styled("code-line", &format!("padding-left:{indent}px"), |p| {
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            p.text("code-text tok-section", trimmed);
            return;
        }
        let tokens = tokens(trimmed);
        let mut command = true;
        for (n, token) in tokens.iter().enumerate() {
            let assigns = tokens.get(n + 1).is_some_and(|next| *next == "=");
            let class = if *token == "&" || *token == "=" || *token == "{" || *token == "}" {
                "tok-punct"
            } else if assigns || token.ends_with(':') {
                command = false;
                "tok-key"
            } else if command {
                command = false;
                "tok-cmd"
            } else if token.starts_with('-') {
                "tok-flag"
            } else if token.starts_with(['"', '\'']) {
                "tok-str"
            } else {
                ""
            };
            p.text(&format!("code-text {class}"), token);
        }
    });
}

/// Whitespace-separated tokens; quoted strings (paths with spaces) stay whole.
fn tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    let mut quote = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => {
                quote = Some(c);
                start.get_or_insert(i);
            }
            (None, c) if c.is_whitespace() => {
                if let Some(s) = start.take() {
                    out.push(&line[s..i]);
                }
            }
            (None, _) => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn tokens_keep_quoted_paths_whole() {
        assert_eq!(
            super::tokens("& 'C:/Program Files/Ara/ara.exe' hub install-mcp --client codex"),
            [
                "&",
                "'C:/Program Files/Ara/ara.exe'",
                "hub",
                "install-mcp",
                "--client",
                "codex"
            ]
        );
        assert_eq!(
            super::tokens("args = [\"mcp\"]"),
            ["args", "=", "[\"mcp\"]"]
        );
    }
}
