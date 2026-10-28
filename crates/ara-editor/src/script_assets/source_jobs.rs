//! Source preparation reads on workers; the authority commits ordinary source.replace ops.
use super::*;
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::FileState;
use ara_host::{file_state::FileHistory, jobs::JobStatus};
use std::sync::mpsc::{self, Receiver};

pub(super) enum Action {
    Create {
        path: String,
        template: templates::Template,
    },
    Rename {
        from: String,
        path: String,
    },
}
impl Action {
    fn path(&self) -> &str {
        match self {
            Self::Create { path, .. } | Self::Rename { path, .. } => path,
        }
    }
    fn previous(&self) -> &str {
        match self {
            Self::Create { path, .. } => path,
            Self::Rename { from, .. } => from,
        }
    }
}
struct Prepared {
    command: Value,
    metadata: Value,
}
enum Stage {
    Waiting {
        action: Action,
        dependency: Option<String>,
    },
    Preparing(Receiver<Result<Prepared, Vec<Diagnostic>>>),
    Applying {
        job: String,
        metadata: Value,
    },
    Compiling {
        job: String,
        metadata: Value,
    },
}
struct Pending {
    session: String,
    path: String,
    doc: Option<String>,
    attach: Vec<String>,
    compile: bool,
    stage: Option<Stage>,
}
#[derive(Default)]
pub(super) struct Authoring {
    pending: BTreeMap<String, Pending>,
    latest: BTreeMap<String, String>,
    tail: Option<String>,
}

/// Both public path forms are accepted; source.replace always receives storage paths.
pub(super) fn storage_path(path: &str) -> Result<String, Vec<Diagnostic>> {
    ara_doc::file_state::check_path(path).map_err(|d| vec![d])?;
    let first = path.split('/').next().unwrap_or("");
    Ok(
        if ara_doc::file_state::same_path(first, ara_doc::AUTHORING_ROOT) {
            path.into()
        } else {
            format!("{}/{path}", ara_doc::AUTHORING_ROOT)
        },
    )
}

impl ScriptService {
    pub(super) fn queue_source(
        &mut self,
        cx: &mut Cx<'_>,
        session: String,
        action: Action,
        doc: Option<String>,
        attach: Vec<String>,
        compile: bool,
    ) -> Result<Value, Vec<Diagnostic>> {
        let path = action.path().to_owned();
        let previous = action.previous().to_owned();
        // Each job prepares against the preceding committed authority revision.
        let dependency = self
            .authoring
            .tail
            .as_ref()
            .filter(|job| {
                cx.job_record(job)
                    .is_some_and(|record| record.status == JobStatus::Running)
            })
            .cloned();
        let class = script_name(&path).map_err(|e| vec![crate::invalid(e)])?;
        let job = cx.start_job("Author C# source");
        self.authoring.tail = Some(job.clone());
        self.authoring
            .latest
            .insert(ara_doc::file_state::path_key(&previous), job.clone());
        self.authoring
            .latest
            .insert(ara_doc::file_state::path_key(&path), job.clone());
        let output = json!({"ok":true,"path":path,"class":class,"job":job,"preparing":true});
        self.authoring.pending.insert(
            job,
            Pending {
                session,
                path,
                doc,
                attach,
                compile,
                stage: Some(Stage::Waiting { action, dependency }),
            },
        );
        Ok(output)
    }

    pub(super) fn poll_sources(&mut self, cx: &mut Cx<'_>) {
        for (job, mut pending) in std::mem::take(&mut self.authoring.pending) {
            if cx.job_cancelled(&job) {
                continue;
            }
            let result = (|| {
                let stage = pending.stage.take().expect("source job has a stage");
                match stage {
                    Stage::Waiting { action, dependency } => {
                        if let Some(parent) = &dependency
                            && cx
                                .job_record(parent)
                                .is_some_and(|record| record.status == JobStatus::Running)
                        {
                            return Ok(Some(Stage::Waiting { action, dependency }));
                        }
                        let root = cx.root().to_owned();
                        let (send, ready) = mpsc::channel();
                        std::thread::Builder::new()
                            .name("ara-script-source".into())
                            .spawn(move || {
                                let _ = send.send(prepare(&root, action));
                            })
                            .map_err(failure)?;
                        cx.job_progress(&job, 0.1, "Preparing source");
                        Ok(Some(Stage::Preparing(ready)))
                    }
                    Stage::Preparing(ready) => {
                        let prepared = match ready.try_recv() {
                            Ok(result) => result?,
                            Err(mpsc::TryRecvError::Empty) => {
                                return Ok(Some(Stage::Preparing(ready)));
                            }
                            Err(_) => return Err(failure("source preparation worker exited")),
                        };
                        let result = cx.resume_session(&pending.session, |cx| {
                            cx.apply(parse(json!({"session":pending.session,"doc":pending.doc,"why":format!("Author C# source {}",pending.path),"commands":[prepared.command]}))?)
                        })??;
                        let source_job = result["job"]
                            .as_str()
                            .ok_or_else(|| failure("source preparation returned no job"))?
                            .to_owned();
                        cx.job_progress(&job, 0.3, "Committing source");
                        Ok(Some(Stage::Applying {
                            job: source_job,
                            metadata: prepared.metadata,
                        }))
                    }
                    Stage::Applying {
                        job: source_job,
                        mut metadata,
                    } => {
                        let Some(result) = child(cx, &source_job)? else {
                            return Ok(Some(Stage::Applying {
                                job: source_job,
                                metadata,
                            }));
                        };
                        metadata["sourceTransaction"] = result;
                        if pending.compile {
                            let compile = cx.resume_session(&pending.session, |cx| {
                                self.start(
                                    cx,
                                    &pending.session,
                                    &pending.path,
                                    pending.doc.clone(),
                                    pending.attach.clone(),
                                )
                                .map_err(failure)
                            })??;
                            cx.job_progress(&job, 0.6, "Compiling source");
                            Ok(Some(Stage::Compiling {
                                job: compile,
                                metadata,
                            }))
                        } else {
                            cx.finish_job(&job, Ok(metadata));
                            Ok(None)
                        }
                    }
                    Stage::Compiling {
                        job: compile,
                        mut metadata,
                    } => {
                        let Some(result) = child(cx, &compile)? else {
                            return Ok(Some(Stage::Compiling {
                                job: compile,
                                metadata,
                            }));
                        };
                        if let (Some(output), Some(result)) =
                            (metadata.as_object_mut(), result.as_object())
                        {
                            output.extend(result.clone());
                        }
                        cx.finish_job(&job, Ok(metadata));
                        Ok(None)
                    }
                }
            })();
            match result {
                Ok(Some(stage)) => {
                    pending.stage = Some(stage);
                    self.authoring.pending.insert(job, pending);
                }
                Ok(None) => {}
                Err(diagnostics) => {
                    for d in &diagnostics {
                        cx.log(d.clone());
                    }
                    cx.finish_job(&job, Err(diagnostics));
                }
            }
        }
    }
    pub(super) fn authoring_source(&self) -> bool {
        !self.authoring.pending.is_empty()
    }
    pub(super) fn pending_source(&self, cx: &Cx<'_>, path: &str) -> bool {
        self.authoring
            .latest
            .get(&ara_doc::file_state::path_key(path))
            .is_some_and(|job| {
                cx.job_record(job)
                    .is_some_and(|record| record.status == JobStatus::Running)
            })
    }
}

fn child(cx: &Cx<'_>, job: &str) -> Result<Option<Value>, Vec<Diagnostic>> {
    let record = cx
        .job_record(job)
        .ok_or_else(|| failure("source dependency job disappeared"))?;
    match record.status {
        JobStatus::Running => Ok(None),
        JobStatus::Done => record
            .result
            .clone()
            .map(Some)
            .ok_or_else(|| failure("completed source job has no result")),
        JobStatus::Failed => Err(record.diagnostics.clone()),
        JobStatus::Cancelled => Err(failure("source dependency was cancelled")),
    }
}
fn failure(error: impl std::fmt::Display) -> Vec<Diagnostic> {
    vec![Diagnostic::error(codes::IO, error.to_string())]
}

fn prepare(root: &Path, action: Action) -> Result<Prepared, Vec<Diagnostic>> {
    let path = action.path();
    let name = script_name(path).map_err(failure)?;
    let history = FileHistory::new(root);
    let (text, namespace, from, base) = match &action {
        Action::Create { path, template } => {
            let parent = root
                .join(path)
                .parent()
                .ok_or_else(|| failure("source needs a directory"))?
                .to_owned();
            let namespace = templates::module_name(root, &parent);
            let text = templates::source(&namespace, name, *template);
            (text, namespace, None, None)
        }
        Action::Rename { from, .. } => {
            let state = history
                .snapshot(from)
                .map_err(|d| vec![d])?
                .ok_or_else(|| failure("source to rename is absent"))?;
            let text =
                String::from_utf8(history.read(&state).map_err(|d| vec![d])?).map_err(failure)?;
            let old = script_name(from).map_err(failure)?;
            (
                rename_identifier(&text, old, name),
                String::new(),
                Some(from.clone()),
                Some(state.hash),
            )
        }
    };
    let file = match &from {
        Some(from) => FileOp::Rename {
            from: from.clone(),
            path: path.into(),
            text: text.clone(),
        },
        None => FileOp::Create {
            path: path.into(),
            text: text.clone(),
        },
    };
    let command = json!({"cmd":"source.replace","args":{"path":path,"from":from,"baseHash":base,"text":text}});
    let metadata = json!({"ok":true,"path":path,"class":name,"namespace":namespace,"files":[file],"hash":FileState::bytes(text.as_bytes()).hash});
    Ok(Prepared { command, metadata })
}
