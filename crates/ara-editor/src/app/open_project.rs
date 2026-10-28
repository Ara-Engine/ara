//! Migration consent before any normal editor authority, scripts or renderer opens.
mod view;
mod window;

use super::Options;
use ara_diag::{Diagnostic, codes};
use ara_host::project_layout::{
    MigrationControl, migrate_project, migration_plan, require_current_layout,
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Preview {
    needs_migration: bool,
    authored_root: String,
    plan_token: String,
    operations: Vec<Value>,
    preview_tree: Vec<String>,
}

pub(super) enum State {
    Checking,
    Ready(Preview),
    Working {
        progress: f32,
        phase: String,
        cancellable: bool,
    },
    Error(Vec<Diagnostic>),
}

impl State {
    fn cancellable(&self) -> bool {
        !matches!(
            self,
            Self::Working {
                cancellable: false,
                ..
            }
        )
    }
}

enum Event {
    Planned(Result<Preview, Vec<Diagnostic>>),
    Progress(f32, String),
    Migrated(Result<Value, Vec<Diagnostic>>),
}

struct Worker {
    events: Receiver<Event>,
    control: MigrationControl,
}

impl Worker {
    fn start(root: PathBuf, token: Option<String>) -> Result<Self, Diagnostic> {
        let (send, events) = mpsc::channel();
        let control = MigrationControl::default();
        let worker_control = control.clone();
        std::thread::Builder::new()
            .name("ara-editor-migration".into())
            .spawn(move || {
                if let Some(token) = token {
                    let result =
                        migrate_project(&root, &token, &worker_control, &mut |progress, phase| {
                            let _ = send.send(Event::Progress(progress, phase.to_owned()));
                        });
                    let _ = send.send(Event::Migrated(result));
                } else {
                    let result = migration_plan(&root).and_then(|plan| {
                        serde_json::to_value(plan)
                            .and_then(serde_json::from_value)
                            .map_err(|error| {
                                vec![Diagnostic::error(codes::INVALID_PARAMS, error.to_string())]
                            })
                    });
                    let _ = send.send(Event::Planned(result));
                }
            })
            .map_err(|error| Diagnostic::error(codes::IO, error.to_string()))?;
        Ok(Self { events, control })
    }
}

/// False means the person cancelled; bounded/headless runs never consent or show UI.
pub(super) fn before_open(options: &Options) -> ara_ui::Result<bool> {
    match require_current_layout(&options.project) {
        Ok(()) => return Ok(true),
        Err(diagnostic) if diagnostic.code == codes::PROJECT_MIGRATION_REQUIRED => {
            if options.offscreen() {
                return Err(format!("{diagnostic}").into());
            }
        }
        Err(diagnostic) => return Err(format!("{diagnostic}").into()),
    }
    let root = options.project.clone();
    let project = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let light = crate::settings::user_settings_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| ara_json::parse(&text).ok())
        .is_some_and(|settings| settings["appearance.theme"] == "light");
    let mut dialog = window::DialogWindow::new(light)?;
    let mut worker =
        Worker::start(root.clone(), None).map_err(|diagnostic| format!("{diagnostic}"))?;
    let mut state = State::Checking;
    let mut cancelled = false;
    let mut changed = true;
    loop {
        loop {
            let event = match worker.events.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if matches!(state, State::Checking | State::Working { .. }) {
                        state = State::Error(vec![Diagnostic::error(
                            codes::IO,
                            "the migration worker stopped before reporting a result; check the project again",
                        )]);
                        changed = true;
                    }
                    break;
                }
            };
            changed = true;
            match event {
                Event::Planned(Ok(preview)) if !preview.needs_migration => return Ok(true),
                Event::Planned(Ok(preview)) => state = State::Ready(preview),
                Event::Planned(Err(diagnostics)) => state = State::Error(diagnostics),
                Event::Progress(progress, phase) => {
                    state = State::Working {
                        progress,
                        phase,
                        cancellable: !cancelled && worker.control.cancellable(),
                    }
                }
                Event::Migrated(Ok(_)) => {
                    require_current_layout(&root).map_err(|diagnostic| format!("{diagnostic}"))?;
                    return Ok(true);
                }
                Event::Migrated(Err(_)) if cancelled => return Ok(false),
                Event::Migrated(Err(diagnostics)) => state = State::Error(diagnostics),
            }
        }
        if let State::Working { cancellable, .. } = &mut state {
            let next = !cancelled && worker.control.cancellable();
            changed |= *cancellable != next;
            *cancellable = next;
        }
        if changed {
            dialog.show(&project, &state)?;
            changed = false;
        }
        for action in dialog.actions(&state)? {
            match action {
                view::Action::Cancel if state.cancellable() => {
                    if matches!(state, State::Working { .. }) {
                        cancelled = worker.control.cancel();
                        state = State::Working {
                            progress: 0.,
                            phase: if cancelled {
                                "Cancelling…"
                            } else {
                                "Finishing files and history…"
                            }
                            .into(),
                            cancellable: false,
                        };
                    } else {
                        return Ok(false);
                    }
                    changed = true;
                }
                view::Action::Migrate => {
                    if let State::Ready(preview) = &state {
                        match Worker::start(root.clone(), Some(preview.plan_token.clone())) {
                            Ok(started) => {
                                worker = started;
                                state = State::Working {
                                    progress: 0.,
                                    phase: "Preparing migration…".into(),
                                    cancellable: true,
                                };
                            }
                            Err(diagnostic) => state = State::Error(vec![diagnostic]),
                        }
                        changed = true;
                    }
                }
                view::Action::Retry => {
                    match Worker::start(root.clone(), None) {
                        Ok(started) => {
                            worker = started;
                            state = State::Checking;
                            cancelled = false;
                        }
                        Err(diagnostic) => state = State::Error(vec![diagnostic]),
                    }
                    changed = true;
                }
                _ => {}
            }
        }
        dialog.frame()?;
        std::thread::sleep(Duration::from_millis(16));
    }
}
