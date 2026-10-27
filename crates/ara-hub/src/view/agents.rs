//! Agents: one-time global MCP setup, then who is attached to each open project.
//! Activity is a feed of presence; nothing here asks anyone to approve anything (D24).
use super::Page;
use crate::model::{AgentClient, Hub, Intent, Participant, Presence, State, text};

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
                "Open a project in the editor or headless, then connect an agent. Editors, people and agents working in each project appear here with what they are doing.",
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
        let (class, icon) = if agent {
            ("avatar avatar-agent", "bot")
        } else {
            ("avatar avatar-human", "user")
        };
        p.panel(class, |p| p.icon(icon, "inherit"));
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

fn connect(p: &mut Page, hub: &Hub) {
    let client = hub.prefs.agent;
    let copied = hub.copied.as_deref();
    p.panel("panel-card", |p| {
        p.panel("row", |p| {
            p.panel("engine-icon", |p| p.icon("plug", "icon-lg accent"));
            p.panel("col fill", |p| {
                p.text("engine-version", "Connect an AI agent");
                p.text(
                    "small muted",
                    "One global MCP server for every project. Install it once per agent client.",
                );
            });
            p.dropdown("agent-client", "Agent client", vec![("Claude Code".into(), Intent::Agent(AgentClient::Claude)), ("Codex".into(), Intent::Agent(AgentClient::Codex))], Some(Intent::Agent(client)));
        });
        p.leaf("divider");
        let Some(executable)=hub.cli_executable.as_ref() else {
            p.text("section-copy","Preparing Ara connection instructions…");
            return;
        };
        let path=executable.to_string_lossy();
        let quoted=if cfg!(windows) {format!("'{}'",path.replace('\'',"''"))}else{format!("'{}'",path.replace('\'',"'\\''"))};
        let invoke=if cfg!(windows){"& "}else{""};
        let printed = hub.install.get(&client).map(String::as_str);
        let install = format!("{invoke}{quoted} hub install-mcp --client {}", client.id());
        match client {
            AgentClient::Claude => {
                step(p, "1", "Register Ara with Claude Code (user scope)", |p| {
                    code(p, copied, "copy-claude-add", &format!("claude mcp add --scope user ara -- {quoted} mcp"));
                });
                step(p, "2", "Or add this entry to ~/.claude.json yourself", |p| {
                    block(p, copied, "copy-claude-json", printed, &install);
                });
            }
            AgentClient::Codex => {
                step(p, "1", "Add this to ~/.codex/config.toml", |p| {
                    block(p, copied, "copy-codex-toml", printed, &install);
                });
                step(p, "2", "Print it again any time", |p| {
                    code(p, copied, "copy-codex-install", &install);
                });
            }
        }
        step(p, "3", "Restart the agent and ask it to list your projects", |p| {
            p.text(
                "section-copy",
                "Agents call ara_projects to list, create, open headless and close projects. They only see projects registered in this Hub; their edits apply immediately and stay undoable per turn.",
            );
        });
    });
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
        p.text("code-text fill", command);
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
                p.text("code-text", line);
            }
        });
        p.copy(copied, id, output.trim_end(), false);
    });
}
