//! Write methods: `cmd.apply`, `jobs.*`, `subscribe`, `doc.sync`, `daemon.shutdown`.

use ara_cmd::{Batch, Call, Kind};
use ara_diag::{Diagnostic, codes};
use ara_doc::Op;
use ara_ops::Author;
use ara_proto::ConnId;
use serde_json::{Map, Value, json};

use super::read::find_doc;
use super::session::checkpoint_at;
use crate::host::{State, session_not_found};
use crate::params::*;

type R = Result<Value, Vec<Diagnostic>>;

/// Rewrites registry locations (`params.x` of call `i`) as `commands[i].args.x`.
fn relocate(diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diags
        .into_iter()
        .map(|mut d| {
            if let (Some(i), Some(path)) = (d.location.command, d.location.path.clone())
                && let Some(rest) = path.strip_prefix("params")
            {
                d.location.path = Some(format!("commands[{i}].args{rest}"));
            }
            d
        })
        .collect()
}

/// Created, modified and deleted records, in first-touch order.
fn diff_summary(ops: &[Op]) -> Value {
    let (mut created, mut modified, mut deleted) = (Vec::new(), Vec::new(), Vec::new());
    for op in ops {
        let id = op.id().to_string();
        match op {
            Op::Create { .. } => created.push(id),
            Op::Delete { .. } => {
                created.retain(|c| *c != id);
                modified.retain(|m| *m != id);
                deleted.push(id);
            }
            Op::Set { .. } | Op::Move { .. } => {
                if !created.contains(&id) && !modified.contains(&id) {
                    modified.push(id);
                }
            }
        }
    }
    json!({ "created": created, "modified": modified, "deleted": deleted })
}

impl State {
    /// `cmd.apply` for session `sid`: one atomic transaction with `why`. Agent turns
    /// get an automatic checkpoint at the revision before them.
    pub(crate) fn apply(&mut self, sid: &str, p: ApplyParams) -> R {
        let session = self
            .sessions
            .live(sid)
            .cloned()
            .ok_or_else(|| vec![session_not_found(sid)])?;
        let db = self.store.schema();
        let mut calls = Vec::new();
        let mut problems = Vec::new();
        for (i, c) in p.commands.iter().enumerate() {
            let mut args = c.args.clone().unwrap_or_else(|| Value::Object(Map::new()));
            if let Some(sym) = &c.symbol {
                let at = |msg: String| {
                    Diagnostic::error(codes::INVALID_PARAMS, msg)
                        .command(i)
                        .path(format!("commands[{i}].as"))
                };
                let creates = self
                    .registry
                    .info(&c.cmd)
                    .and_then(|info| db.get(&info.params))
                    .is_some_and(|t| t.field("id").is_some());
                if !sym.starts_with('$') {
                    problems.push(at(format!("symbol `{sym}` must start with `$`")));
                } else if !creates {
                    problems.push(at(format!(
                        "`{}` does not create a record, so `as` has nothing to name",
                        c.cmd
                    )));
                } else if let Some(obj) = args.as_object_mut() {
                    if obj.contains_key("id") {
                        problems.push(at("give `as` or `args.id`, not both".into()));
                    } else {
                        obj.insert("id".into(), Value::String(sym.clone()));
                    }
                }
            }
            calls.push(Call {
                command: c.cmd.clone(),
                params: args,
            });
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        let dry_run = p.dry_run.unwrap_or(false);
        let batch = Batch {
            doc: p.doc,
            calls,
            why: p.why.clone(),
            idempotency_key: p.idempotency_key,
            base: p.base,
            dry_run,
        };
        let before = self.store.revision();
        let result = self
            .registry
            .apply(&mut self.store, &batch, &session.cmd_session())
            .map_err(relocate)?;
        let turn = matches!(session.author, Author::Agent(_))
            && !dry_run
            && !result.replayed
            && result.txn.is_some();
        let checkpoint = turn.then(|| checkpoint_at(self, sid, before, p.why.clone(), true).id);
        Ok(json!({
            "ok": true,
            "dryRun": result.dry_run,
            "txn": result.txn,
            "replayed": result.replayed,
            "revision": result.revision,
            "ids": result.symbols,
            "diff": diff_summary(&result.ops),
            "outputs": result.outputs,
            "ops": result.ops,
            "checkpoint": checkpoint,
        }))
    }

    pub(crate) fn start_job(&mut self, name: &str, session: Option<String>) -> String {
        let id = self.token("jb");
        let record = json!(self.jobs.start(id.clone(), name, session));
        self.notify(Topic::Jobs, record);
        id
    }

    pub(crate) fn job_progress(&mut self, id: &str, fraction: f32, message: &str) {
        if let Some(r) = self.jobs.progress(id, fraction, message) {
            let r = json!(r);
            self.notify(Topic::Jobs, r);
        }
    }

    pub(crate) fn finish_job(&mut self, id: &str, result: Result<Value, Vec<Diagnostic>>) {
        if let Some(r) = self.jobs.finish(id, result) {
            let r = json!(r);
            self.notify(Topic::Jobs, r);
        }
    }
}

pub(super) fn jobs_list(st: &mut State) -> Value {
    let jobs: Vec<&crate::jobs::JobRecord> = st.jobs.all().collect();
    json!({ "jobs": jobs })
}

fn job_missing(id: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        codes::JOB_NOT_FOUND,
        format!("no job `{id}`"),
    )]
}

pub(super) fn job_status(st: &mut State, p: JobParams) -> R {
    st.jobs
        .get(&p.job)
        .map(|j| json!(j))
        .ok_or_else(|| job_missing(&p.job))
}

pub(super) fn job_cancel(st: &mut State, p: JobParams) -> R {
    if st.jobs.get(&p.job).is_none() {
        return Err(job_missing(&p.job));
    }
    let cancelled = st.jobs.cancel(&p.job);
    if let Some(j) = st.jobs.get(&p.job) {
        let j = json!(j);
        st.notify(Topic::Jobs, j);
    }
    Ok(json!({ "job": p.job, "cancelled": cancelled }))
}

/// Runs a registry job in one transaction, then reports its progress and result.
pub(super) fn job_start(st: &mut State, sid: &str, p: JobStartParams) -> R {
    let session = st
        .sessions
        .live(sid)
        .cloned()
        .ok_or_else(|| vec![session_not_found(sid)])?;
    match st.registry.info(&p.job).map(|i| i.kind) {
        Some(Kind::Job) => {}
        Some(other) => {
            return Err(vec![Diagnostic::error(
                codes::WRONG_KIND,
                format!("`{}` is a {other:?}, not a job; use cmd.apply", p.job),
            )]);
        }
        None => {
            return Err(vec![Diagnostic::error(
                codes::UNKNOWN_COMMAND,
                format!(
                    "no job `{}`; list them with schema.list {{\"kind\": \"job\"}}",
                    p.job
                ),
            )]);
        }
    }
    let id = st.start_job(&p.job, Some(sid.to_owned()));
    let mut batch = Batch::new(p.doc, []);
    batch.calls.push(Call {
        command: p.job.clone(),
        params: p.args.unwrap_or_else(|| Value::Object(Map::new())),
    });
    batch.why = p.why;
    let mut steps: Vec<(f32, String)> = Vec::new();
    let result = st.registry.run(
        &mut st.store,
        &batch,
        &session.cmd_session(),
        &mut |f, m| steps.push((f, m.to_owned())),
    );
    for (f, m) in steps {
        st.job_progress(&id, f, &m);
    }
    let outcome = result.map(
        |r| json!({ "txn": r.txn, "revision": r.revision, "output": r.outputs.into_iter().next() }),
    );
    st.finish_job(&id, outcome);
    st.jobs
        .get(&id)
        .map(|j| json!(j))
        .ok_or_else(|| job_missing(&id))
}

pub(super) fn subscribe(st: &mut State, conn: ConnId, p: SubscribeParams, on: bool) -> Value {
    let topics = st.subs.entry(conn).or_default();
    for t in p.topics {
        if on {
            topics.insert(t);
        } else {
            topics.remove(&t);
        }
    }
    let now: Vec<Topic> = topics.iter().copied().collect();
    json!({ "topics": now })
}

pub(super) fn doc_sync(st: &mut State, p: DocSyncParams) -> R {
    let id = find_doc(st, &p.doc)?;
    st.sync(&id, p.keep)
}

pub(super) fn shutdown(st: &mut State, sid: &str) -> R {
    let human = st
        .sessions
        .live(sid)
        .is_some_and(|s| s.author == Author::Human);
    if !human {
        return Err(vec![Diagnostic::error(
            codes::MISSING_SCOPE,
            "only human sessions can stop the daemon",
        )]);
    }
    st.drain();
    st.flush(true);
    st.stop = true;
    Ok(json!({ "stopping": true }))
}
