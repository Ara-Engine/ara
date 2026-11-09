//! Connect an agent: the Hub's per-client setup inside the editor. Each client
//! (Claude Code, Codex, Cursor, OpenCode) shows its mark and what was detected,
//! connects in one click, and offers manual steps only where it can't be connected.
use super::*;
use ara_agents::{Client, Setup};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, channel},
};

/// What a client row's button does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AgentAction {
    /// Register Ara with the client (or retry after a failure).
    Connect,
    /// Show or hide the client's manual steps.
    Manual,
    /// Copy the manual step.
    Copy,
}

/// Detection and setup results, delivered by background probes.
pub(crate) struct AgentSetups {
    setup: BTreeMap<Client, Setup>,
    manual: Option<Client>,
    /// The `ara` CLI that agents launch (`<ara> mcp`), beside the editor.
    ara: Option<PathBuf>,
    send: Sender<(Client, Setup)>,
    receive: Receiver<(Client, Setup)>,
}

impl Default for AgentSetups {
    fn default() -> Self {
        let (send, receive) = channel();
        Self {
            setup: BTreeMap::new(),
            manual: None,
            ara: None,
            send,
            receive,
        }
    }
}

/// The engine build's `ara` CLI, installed beside the editor.
fn ara_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let ara = exe.with_file_name(format!("ara{}", std::env::consts::EXE_SUFFIX));
    ara.is_file().then(|| dunce::simplified(&ara).to_path_buf())
}

impl Shell {
    /// Open the dialog and probe every client again (they may have changed since).
    pub(crate) fn open_agents(&mut self) {
        let agents = &mut self.agents;
        agents.setup.clear();
        agents.manual = None;
        agents.ara = ara_cli();
        for client in Client::ALL {
            let send = agents.send.clone();
            // One thread each: every probe waits on the client's CLI.
            std::thread::spawn(move || {
                let _ = send.send((client, ara_agents::detect(client)));
            });
        }
        self.dialog = Some(Dialog::Agents);
    }

    /// Apply finished probes and setups; true when the dialog should redraw.
    pub(crate) fn poll_agents(&mut self, _editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = false;
        for (client, setup) in self.agents.receive.try_iter() {
            self.agents.setup.insert(client, setup);
            changed = true;
        }
        Ok(changed && matches!(self.dialog, Some(Dialog::Agents)))
    }

    /// Whether every client's probe has answered (captures wait for it).
    #[cfg(test)]
    pub(super) fn agents_detected(&self) -> bool {
        self.agents.setup.len() == Client::ALL.len()
    }

    pub(super) fn agent_action(&mut self, client: Client, action: AgentAction) {
        let agents = &mut self.agents;
        match action {
            AgentAction::Connect => {
                let Some(ara) = agents.ara.clone() else {
                    return;
                };
                agents.setup.insert(client, Setup::Working);
                let send = agents.send.clone();
                std::thread::spawn(move || {
                    let _ = send.send((client, ara_agents::setup(client, &ara)));
                });
            }
            AgentAction::Manual => {
                agents.manual = (agents.manual != Some(client)).then_some(client);
            }
            AgentAction::Copy => {
                let ara = agents.ara.clone().unwrap_or_else(|| PathBuf::from("ara"));
                self.copy_text(ara_agents::manual(&ara, client));
                self.status_note = Some(format!("Copied the {} setup", client.name()));
            }
        }
    }

    pub(super) fn agents_markup(&mut self, out: &mut String) {
        let connected: Vec<_> = Client::ALL
            .into_iter()
            .filter(|client| self.agents.setup.get(client) == Some(&Setup::Connected))
            .collect();
        out.push_str("<panel id='agents-head' class='agents-head'><panel id='agents-head-logo' class='agent-logo'>");
        out.push_str(&icons::icon_in("agents-head", "plug", "accent"));
        out.push_str("</panel><panel id='agents-head-text' class='agent-text'><text id='dialog-title' class='title' text='Connect AI agents' />");
        out.push_str(&format!("<text id='dialog-line-0' class='dialog-text' text='{}' /></panel></panel>", escape("One MCP server for every project in your Hub. Agents open this project and edit it with the same commands you use; each change is undoable.")));
        out.push_str("<scroll id='dialog-content' style='flex-basis:auto;min-height:0px;flex-shrink:1;gap:6px'>");
        for client in Client::ALL {
            self.agent_row(client, out);
        }
        out.push_str("</scroll>");
        let note = if self.agents.ara.is_none() {
            Some(
                "Ara's command-line tool isn't beside this editor, so agents can only be set up by hand. The steps assume ara is on your PATH.",
            )
        } else if !connected.is_empty() {
            Some("Restart a newly connected agent, then ask it to list your projects.")
        } else {
            None
        };
        if let Some(note) = note {
            out.push_str(&format!(
                "<text id='agents-note' class='dialog-text' text='{}' />",
                escape(note)
            ));
        }
        self.dialog_buttons(None, out);
    }

    /// One client: its mark, what was detected, and the one action that fits.
    fn agent_row(&mut self, client: Client, out: &mut String) {
        let id = format!("agent-{}", client.id());
        let state = self.agents.setup.get(&client).cloned();
        let manual = self.agents.manual == Some(client);
        let can_connect = self.agents.ara.is_some();
        // Claude keeps its brand colour; the monochrome marks follow the text colour.
        let tint = if client == Client::Claude {
            ""
        } else {
            " tint='true'"
        };
        out.push_str(&format!("<panel id='{id}' class='agent-client'><panel id='{id}-row' class='agent-row'><panel id='{id}-logo' class='agent-logo'><image id='{id}-mark' class='agent-mark' src='icon-agent-{}'{tint} /></panel>", client.id()));
        let status = match &state {
            None => "Looking for it…",
            Some(Setup::Missing) => "Not found on this computer",
            Some(Setup::Available) => "Installed · not connected yet",
            Some(Setup::Working) => "Connecting…",
            Some(Setup::Connected) => "Ara is connected",
            Some(Setup::Failed(_)) => "Couldn't connect",
        };
        out.push_str(&format!("<panel id='{id}-text' class='agent-text'><text id='{id}-name' class='agent-name' text='{}' /><text id='{id}-status' class='agent-status' text='{status}' /></panel>", escape(client.name())));
        match &state {
            Some(Setup::Available) if can_connect => out.push_str(&self.bind_button(
                &format!("{id}-connect"),
                "Connect",
                Binding::Agent(client, AgentAction::Connect),
                "primary",
            )),
            Some(Setup::Failed(_)) if can_connect => out.push_str(&self.bind_button(
                &format!("{id}-retry"),
                "Retry",
                Binding::Agent(client, AgentAction::Connect),
                "",
            )),
            Some(Setup::Connected) => out.push_str(&format!("<panel id='{id}-pill' class='chip-success agent-pill'><panel id='{id}-dot' class='status-dot' /><text id='{id}-pill-text' text='Connected' /></panel>")),
            None | Some(Setup::Working) => {
                out.push_str(&icons::icon(&format!("{id}-busy"), "loader-circle"))
            }
            _ => {}
        }
        // Manual steps only where the editor can't connect the client itself.
        let offer_manual = match &state {
            Some(Setup::Missing | Setup::Failed(_)) => true,
            Some(Setup::Available) => !can_connect,
            _ => false,
        };
        if offer_manual {
            out.push_str(&self.bind_button(
                &format!("{id}-manual"),
                if manual {
                    "Hide steps"
                } else {
                    "Set up manually"
                },
                Binding::Agent(client, AgentAction::Manual),
                "",
            ));
        }
        out.push_str("</panel>");
        if let Some(Setup::Failed(error)) = &state {
            out.push_str(&format!(
                "<text id='{id}-error' class='agent-error' text='{}' />",
                escape(error)
            ));
        }
        if manual {
            let ara = self
                .agents
                .ara
                .clone()
                .unwrap_or_else(|| PathBuf::from("ara"));
            out.push_str(&format!("<panel id='{id}-steps' class='agent-steps'><panel id='{id}-hint-row' class='agent-row'><text id='{id}-hint' class='agent-status agent-text' text='{}' />", escape(client.manual_hint())));
            out.push_str(&self.bind_button(
                &format!("{id}-copy"),
                "Copy",
                Binding::Agent(client, AgentAction::Copy),
                "",
            ));
            out.push_str(&format!("</panel><text id='{id}-code' class='dialog-code' text='{}' /><text id='{id}-restart' class='agent-status' text='Then restart the agent and ask it to list your projects.' /></panel>", escape(ara_agents::manual(&ara, client).trim_end())));
        }
        out.push_str("</panel>");
    }
}
