//! Compatibility source-write endpoint over the shared prepared, reversible command.
use super::*;
use ara_host::{file_state::FileHistory, jobs::JobStatus, parse};
use ara_script::{SourceClaim, SourceClaimPhase};
use std::sync::mpsc::{self, Receiver};

struct Prepared {
    path: String,
    hash: String,
    next_hash: String,
    editor: bool,
}
struct Pending {
    params: ScriptWriteParams,
    agent: bool,
    job: String,
    ready: Receiver<Result<Prepared, Vec<Diagnostic>>>,
    child: Option<(String, usize, String)>,
    claim: Option<(String, SourceClaim)>,
}
#[derive(Default)]
pub(crate) struct SourceWrites {
    pending: Vec<Pending>,
}

impl PlayService {
    pub(crate) fn queue_script_write(
        &mut self,
        cx: &mut Cx<'_>,
        params: ScriptWriteParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        ara_doc::file_state::check_path(&params.path).map_err(|d| vec![d])?;
        let root = self.root.clone();
        let path = params.path.clone();
        let text = params.text.clone();
        let (send, ready) = mpsc::channel();
        std::thread::Builder::new()
            .name("ara-script-write".into())
            .spawn(move || {
                let result = (|| {
                    let inventory = ara_script::project::inventory(&root).map_err(|d| vec![d])?;
                    if !inventory.diagnostics.is_empty() {
                        return Err(inventory.diagnostics);
                    }
                    let source = inventory
                        .sources
                        .iter()
                        .find(|source| {
                            ara_doc::file_state::same_path(&source.storage_path, &path)
                                || ara_doc::file_state::same_path(&source.path, &path)
                        })
                        .ok_or_else(|| {
                            fail(
                                codes::REQUEST,
                                format!("{path} is not an inventoried C# source"),
                            )
                        })?;
                    let state = FileHistory::new(&root)
                        .snapshot(&source.storage_path)
                        .map_err(|d| vec![d])?
                        .ok_or_else(|| fail(codes::REQUEST, "source disappeared"))?;
                    Ok(Prepared {
                        path: source.storage_path.clone(),
                        hash: state.hash,
                        next_hash: ara_doc::file_state::FileState::bytes(text.as_bytes()).hash,
                        editor: source.editor,
                    })
                })();
                let _ = send.send(result);
            })
            .map_err(|e| fail(codes::REQUEST, e.to_string()))?;
        let agent = cx
            .session()
            .is_some_and(|session| session.author.to_string().starts_with("agent:"));
        let job = cx.start_job("Write C# source");
        let output = json!({"ok":true,"path":params.path,"agent":agent,"job":job,"preparing":true});
        self.source_writes.pending.push(Pending {
            params,
            agent,
            job,
            ready,
            child: None,
            claim: None,
        });
        Ok(output)
    }

    pub(crate) fn poll_script_writes(&mut self, cx: &mut Cx<'_>) {
        for mut pending in std::mem::take(&mut self.source_writes.pending) {
            if cx.job_cancelled(&pending.job) {
                // A committed file batch is noncancellable. Keep its version claim
                // until its worker finishes, even if the parent was cancelled.
                let materializing = pending.child.as_ref().is_some_and(|(child, _, _)| {
                    !cx.cancel_job(child)
                        && cx.job_record(child).is_some_and(|record| {
                            matches!(record.status, JobStatus::Running | JobStatus::Done)
                        })
                });
                if !materializing {
                    self.retire_source_claim(cx, &mut pending, SourceClaimPhase::Cancelled);
                    continue;
                }
            }
            let result = (|| {
                if let Some((child, claimed, path)) = &pending.child {
                    let record = cx
                        .job_record(child)
                        .ok_or_else(|| fail(codes::REQUEST, "source job disappeared"))?;
                    return match record.status {
                        JobStatus::Running => Ok(None),
                        JobStatus::Done => {
                            let mut result = record
                                .result
                                .clone()
                                .ok_or_else(|| fail(codes::REQUEST, "source job has no result"))?;
                            result["path"] = path.clone().into();
                            result["agent"] = pending.agent.into();
                            result["claimedBy"] = (*claimed).into();
                            Ok(Some(result))
                        }
                        JobStatus::Failed => Err(record.diagnostics.clone()),
                        JobStatus::Cancelled => Err(fail(codes::REQUEST, "source job cancelled")),
                    };
                }
                let prepared = match pending.ready.try_recv() {
                    Ok(result) => result?,
                    Err(mpsc::TryRecvError::Empty) => return Ok(None),
                    Err(_) => return Err(fail(codes::REQUEST, "source worker exited")),
                };
                if pending.agent && !prepared.editor {
                    pending.claim = Some((
                        prepared.path.clone(),
                        SourceClaim {
                            edit: pending.job.clone(),
                            hash: prepared.next_hash,
                            phase: SourceClaimPhase::Preparing,
                        },
                    ));
                }
                let child = cx.resume_session(&pending.params.session, |cx| {
                    let claimed = match &pending.claim {
                        Some((path, claim)) => self.claim_source(&self.root.join(path), claim)?,
                        None => 0,
                    };
                    let result = cx.apply(parse(json!({"session":pending.params.session,"why":format!("Write C# source {}",prepared.path),"commands":[{"cmd":"source.replace","args":{"path":prepared.path,"baseHash":prepared.hash,"text":pending.params.text}}]}))?)?;
                    Ok::<_,Vec<Diagnostic>>((result["job"].as_str().ok_or_else(||fail(codes::REQUEST,"source command returned no job"))?.to_owned(),claimed,prepared.path))
                })??;
                pending.child = Some(child);
                cx.job_progress(&pending.job, 0.5, "Committing source");
                Ok(None)
            })();
            match result {
                Ok(None) => self.source_writes.pending.push(pending),
                Ok(Some(result)) => {
                    self.retire_source_claim(cx, &mut pending, SourceClaimPhase::Materialized);
                    cx.finish_job(&pending.job, Ok(result));
                }
                Err(diagnostics) => {
                    self.retire_source_claim(cx, &mut pending, SourceClaimPhase::Cancelled);
                    for diagnostic in &diagnostics {
                        cx.log(diagnostic.clone());
                    }
                    cx.finish_job(&pending.job, Err(diagnostics));
                }
            }
        }
    }

    fn retire_source_claim(
        &mut self,
        cx: &mut Cx<'_>,
        pending: &mut Pending,
        phase: SourceClaimPhase,
    ) {
        if let Some((path, mut claim)) = pending.claim.take() {
            claim.phase = phase;
            if let Err(diagnostics) = self.claim_source(&self.root.join(path), &claim) {
                for diagnostic in diagnostics {
                    cx.log(diagnostic);
                }
            }
        }
    }
}
