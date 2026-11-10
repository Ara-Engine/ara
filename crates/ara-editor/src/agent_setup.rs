//! Connecting AI agents to Ara from the editor, as the Hub does: each client (Claude
//! Code, Codex, Cursor, OpenCode) is detected and set up on its own background thread
//! (`ara_agents`), and gets manual steps where the editor cannot connect it itself.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use ara_agents::{Client, Setup};

/// Detection and setup results, delivered by background probes.
pub(crate) struct AgentSetups {
    /// What each client's latest probe or setup said.
    pub setup: BTreeMap<Client, Setup>,
    /// The client whose manual steps show.
    pub manual: Option<Client>,
    /// The `ara` CLI that agents launch (`<ara> mcp`), beside the editor.
    pub ara: Option<PathBuf>,
    /// Whether the clients have been looked for since the editor opened.
    pub probed: bool,
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
            probed: false,
            send,
            receive,
        }
    }
}

impl std::fmt::Debug for AgentSetups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSetups")
            .field("setup", &self.setup)
            .field("manual", &self.manual)
            .finish()
    }
}

/// The engine build's `ara` CLI, installed beside the editor.
pub(crate) fn ara_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let ara = exe.with_file_name(format!("ara{}", std::env::consts::EXE_SUFFIX));
    ara.is_file().then(|| dunce::simplified(&ara).to_path_buf())
}

impl AgentSetups {
    /// Looks for every client again (they may have changed since), one thread each: every
    /// probe waits on the client's own CLI.
    pub(crate) fn probe(&mut self) {
        self.setup.clear();
        self.manual = None;
        self.ara = ara_cli();
        self.probed = true;
        for client in Client::ALL {
            let send = self.send.clone();
            std::thread::spawn(move || {
                let _ = send.send((client, ara_agents::detect(client)));
            });
        }
    }

    /// Takes finished probes and setups; whether any arrived.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        for (client, setup) in self.receive.try_iter() {
            self.setup.insert(client, setup);
            changed = true;
        }
        changed
    }

    /// Registers Ara with `client` in the background (or retries after a failure).
    pub(crate) fn connect(&mut self, client: Client) {
        let Some(ara) = self.ara.clone() else {
            return;
        };
        self.setup.insert(client, Setup::Working);
        let send = self.send.clone();
        std::thread::spawn(move || {
            let _ = send.send((client, ara_agents::setup(client, &ara)));
        });
    }

    /// Shows or hides `client`'s manual steps.
    pub(crate) fn toggle_manual(&mut self, client: Client) {
        self.manual = (self.manual != Some(client)).then_some(client);
    }

    /// `client`'s manual setup, for the clipboard (assumes `ara` on the PATH when it is
    /// not beside the editor).
    pub(crate) fn manual_text(&self, client: Client) -> String {
        let ara = self.ara.clone().unwrap_or_else(|| PathBuf::from("ara"));
        ara_agents::manual(&ara, client)
    }

    /// Whether `client`'s manual steps are worth offering: the editor cannot connect it.
    pub(crate) fn offers_manual(&self, client: Client) -> bool {
        match self.setup.get(&client) {
            Some(Setup::Missing | Setup::Failed(_)) => true,
            Some(Setup::Available) => self.ara.is_none(),
            _ => false,
        }
    }
}
