//! Cached authority metadata and OS lifetime notifications.
use super::*;
use std::sync::mpsc::{Receiver, Sender};

pub(super) struct Exited {
    project: String,
    generation: u64,
}
pub(super) struct Running {
    pub(super) info: DaemonInfo,
    pub(super) headless: bool,
    generation: u64,
    pub(super) watch: super::process::Watch,
}
pub(super) struct Processes {
    pub(super) running: BTreeMap<String, Running>,
    retired: BTreeMap<String, (u32, u16, String)>,
    exits: Receiver<Exited>,
    send: Sender<Exited>,
    generation: u64,
}
impl Default for Processes {
    fn default() -> Self {
        let (send, exits) = std::sync::mpsc::channel();
        Self {
            running: BTreeMap::new(),
            retired: BTreeMap::new(),
            exits,
            send,
            generation: 0,
        }
    }
}
impl Processes {
    pub(super) fn forget(&mut self, project: &str) {
        if let Some(running) = self.running.remove(project) {
            self.retired.insert(
                project.to_owned(),
                (running.info.pid, running.info.port, running.info.token),
            );
        }
    }
}
impl Hub {
    fn retired_process(&mut self, project: &str) {
        self.processes.forget(project);
        self.routes.retain(|(_, id), _| id != project);
        self.activity_clients.remove(project);
        if let Some(mut child) = self.children.remove(project) {
            let _ = child.try_wait();
        }
        self.changed("closed", Some(project));
    }
    pub(super) fn poll_processes(&mut self) {
        // Unix keeps exited children as zombies until their parent reaps them.
        // Windows uses signalled process handles and needs no polling/reaping lane.
        #[cfg(not(windows))]
        for child in self.children.values_mut() {
            let _ = child.try_wait();
        }
        let exited: Vec<_> = self.processes.exits.try_iter().collect();
        for exit in exited {
            // A previous authority's exit can arrive after a headless/editor handoff.
            if self
                .processes
                .running
                .get(&exit.project)
                .is_none_or(|running| running.generation != exit.generation)
            {
                continue;
            }
            self.retired_process(&exit.project);
        }
    }

    /// Discovery is consulted on explicit reads/open and service startup only.
    /// Once authenticated, process lifetime comes from a kernel wait.
    pub(super) fn discover_process(&mut self, project: &ProjectEntry) -> Result<(), RpcError> {
        if let Some(running) = self.processes.running.get(&project.id) {
            if !running.watch.exited() {
                return Ok(());
            }
            // A kernel wait can signal before its message reaches the authority.
            // Retire that identity before discovering a replacement process.
            self.retired_process(&project.id);
        }
        let Some(info) = DaemonInfo::read(&project.path) else {
            return Ok(());
        };
        if self
            .processes
            .retired
            .get(&project.id)
            .is_some_and(|old| *old == (info.pid, info.port, info.token.clone()))
        {
            return Ok(());
        }
        let generation = self.processes.generation;
        self.processes.generation += 1;
        let send = self.processes.send.clone();
        let id = project.id.clone();
        let watch = match super::process::Watch::new(info.pid, move || {
            let _ = send.send(Exited {
                project: id,
                generation,
            });
        }) {
            Ok(watch) => watch,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(87) =>
            {
                return Ok(());
            }
            Err(error) => {
                return Err(io_error(format!(
                    "cannot observe project process {}: {error}",
                    info.pid
                )));
            }
        };
        let Ok(mut client) = Client::connect(info.port, &info.token) else {
            return Ok(());
        };
        let Ok(catalog) = client.call("rpc.discover", json!({})) else {
            return Ok(());
        };
        if watch.exited() {
            return Ok(());
        }
        let headless = !catalog["methods"].as_array().is_some_and(|methods| {
            methods
                .iter()
                .any(|method| method["name"] == "editor.state")
        });
        self.processes.retired.remove(&project.id);
        self.processes.running.insert(
            project.id.clone(),
            Running {
                info,
                headless,
                generation,
                watch,
            },
        );
        self.changed("opened", Some(&project.id));
        Ok(())
    }
}
