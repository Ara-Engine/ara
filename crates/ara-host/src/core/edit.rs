//! Write methods: `cmd.apply`, `jobs.*`, `subscribe`, `doc.sync`, `daemon.shutdown`.

use std::collections::BTreeMap;

use ara_cmd::select::references;
use ara_cmd::{Batch, BatchResult, Call, Kind};
use ara_diag::{Diagnostic, FixAction, codes};
use ara_doc::{AssetId, Op, Record};
use ara_ops::Author;
use ara_proto::ConnId;
use serde_json::{Map, Value, json};

use super::pick::{find_doc, path_of, pick};
use super::session::checkpoint_at;
use crate::host::{State, session_not_found};
use crate::params::*;

type R = Result<Value, Vec<Diagnostic>>;

/// Rewrites registry locations (`params.x` of call `i`) as `commands[i].args.x`, in
/// the location and in the paths of `set`/`remove`/`rename` fixes.
pub(crate) fn relocate(diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let moved = |i: usize, path: &str| {
        path.strip_prefix("params")
            .map(|rest| format!("commands[{i}].args{rest}"))
    };
    diags
        .into_iter()
        .map(|mut d| {
            let Some(i) = d.location.command else {
                return d;
            };
            if let Some(p) = d.location.path.as_deref().and_then(|p| moved(i, p)) {
                d.location.path = Some(p);
            }
            for fix in &mut d.fixes {
                if let FixAction::Set { path, .. }
                | FixAction::Remove { path, .. }
                | FixAction::Rename { path, .. } = &mut fix.action
                    && let Some(p) = moved(i, path)
                {
                    *path = p;
                }
            }
            d
        })
        .collect()
}

/// Lists every command and query (`name(arg, arg?)`: summary) in the description of
/// `cmd`'s `cmd` field, so a client sees the commands and their arguments without a
/// schema lookup. Generated from the registry, so it follows registrations.
pub(crate) fn list_commands(
    schema: &mut Value,
    registry: &ara_cmd::Registry,
    db: &ara_schema::SchemaDb,
) {
    let Some(cmd) = schema.pointer_mut("/$defs/CallSpec/properties/cmd") else {
        return;
    };
    let mut text = String::from(
        "Command name. Built in (args; `?` = optional; entity args take IDs, `$symbols` or `name:Crate` selectors; `ara_schema describe` gives full schemas):",
    );
    for info in registry.infos().filter(|i| i.kind != Kind::Job) {
        let args: Vec<String> = db
            .get(&info.params)
            .map(|t| {
                t.fields()
                    .iter()
                    .map(|f| format!("{}{}", f.name, if f.required { "" } else { "?" }))
                    .collect()
            })
            .unwrap_or_default();
        text.push_str(&format!(
            "\n- `{}({})`: {}",
            info.name,
            args.join(", "),
            info.summary
        ));
    }
    cmd["description"] = text.into();
}

/// `name` of a record, or the prefab it instantiates (`crate instance`).
fn label(record: Option<&Record>) -> String {
    let Some(r) = record else {
        return String::new();
    };
    if let Some(n) = r.get("name").and_then(Value::as_str) {
        return n.to_owned();
    }
    match r.get("prefab").and_then(Value::as_str) {
        Some(p) => {
            let file = p.trim_start_matches('@').split('#').next().unwrap_or(p);
            let stem = file.rsplit('/').next().unwrap_or(file);
            format!("{} instance", stem.split('.').next().unwrap_or(stem))
        }
        None => "unnamed".into(),
    }
}

/// The text form of a `cmd.apply` result: what was applied where, and every created,
/// modified and deleted record with its name and symbol.
pub(crate) fn apply_text(
    st: &State,
    doc: &AssetId,
    r: &BatchResult,
    checkpoint: Option<&str>,
) -> String {
    let path = path_of(st, doc);
    let current = st.store.workspace().doc(doc.as_str());
    let symbols: BTreeMap<&str, &str> = r
        .symbols
        .iter()
        .map(|(sym, id)| (id.as_str(), sym.as_str()))
        .collect();
    let (mut created, mut modified, mut deleted) = (Vec::new(), Vec::new(), Vec::new());
    for op in &r.ops {
        let id = op.id().as_str();
        let line = |rec: Option<&Record>| {
            let mut s = format!("{id} {}", label(rec));
            if let Some(sym) = symbols.get(id) {
                s.push_str(&format!(" ({sym})"));
            }
            s
        };
        let here = current.and_then(|d| d.record(id));
        match op {
            Op::Create { record, .. } => created.push((id, line(here.or(Some(record))))),
            Op::Delete { snapshot, .. } => {
                created.retain(|(c, _)| *c != id);
                modified.retain(|(m, _)| *m != id);
                deleted.push((id, line(Some(snapshot))));
            }
            Op::Set { .. } | Op::Move { .. } => {
                if !created.iter().chain(&modified).any(|(c, _)| *c == id) {
                    modified.push((id, line(here)));
                }
            }
        }
    }
    let mut out = match (&r.txn, r.dry_run) {
        (_, true) => format!("dry run on {path}: nothing was applied"),
        (Some(t), false) => format!("applied {t} to {path} (revision {})", r.revision),
        (None, false) => format!("no changes to {path}"),
    };
    let verbs = if r.dry_run {
        ["would create", "would modify", "would delete"]
    } else {
        ["created", "modified", "deleted"]
    };
    for (verb, list) in verbs.iter().zip([&created, &modified, &deleted]) {
        if !list.is_empty() {
            let names: Vec<&str> = list.iter().map(|(_, l)| l.as_str()).collect();
            out.push_str(&format!("\n  {verb}: {}", names.join(", ")));
        }
    }
    if let Some(cp) = checkpoint {
        out.push_str(&format!(
            "\nundo this turn: ara_session {{\"action\": \"rollback\", \"to\": \"{cp}\"}}"
        ));
    }
    out.push('\n');
    out
}

/// Created, modified and deleted records, in first-touch order.
pub(crate) fn diff_summary(ops: &[Op]) -> Value {
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
    /// The document and batch a `cmd.apply` call runs: document inference, `as`
    /// symbols and their checks. Branch applies (`collab`) share it.
    pub(crate) fn batch(&self, p: &ApplyParams) -> Result<(AssetId, Batch), Vec<Diagnostic>> {
        let db = self.store.schema();
        let dry_run = p.dry_run.unwrap_or(false);
        let refs: Vec<String> = p
            .commands
            .iter()
            .filter_map(|c| {
                let info = self.registry.info(&c.cmd)?;
                Some(references(db, &info.params, c.args.as_ref()?))
            })
            .flatten()
            .collect();
        let doc = pick(self, p.doc.as_deref(), &refs)?;
        let mut calls = Vec::new();
        let mut problems = Vec::new();
        for (i, c) in p.commands.iter().enumerate() {
            let mut args = c.args.clone().unwrap_or_else(|| Value::Object(Map::new()));
            // `as: "crate"` means `$crate`, the name later calls use.
            let symbol = c.symbol.as_ref().map(|s| {
                if s.starts_with('$') {
                    s.clone()
                } else {
                    format!("${s}")
                }
            });
            if let Some(sym) = &symbol {
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
                if !creates {
                    problems.push(at(format!(
                        "`{}` does not create a record, so `as` has nothing to name",
                        c.cmd
                    )));
                } else if let Some(obj) = args.as_object_mut() {
                    match obj.get("id") {
                        Some(id) if id.as_str() == Some(sym.as_str()) => {}
                        Some(_) => problems.push(at(
                            "`as` and `args.id` name different IDs; give one of them".into(),
                        )),
                        None => {
                            obj.insert("id".into(), Value::String(sym.clone()));
                        }
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
        let batch = Batch {
            doc: doc.to_string(),
            calls,
            why: p.why.clone(),
            idempotency_key: p.idempotency_key.clone(),
            base: p.base,
            dry_run,
        };
        Ok((doc, batch))
    }

    /// `cmd.apply` for session `sid`: one atomic transaction with `why`. Agent turns
    /// get an automatic checkpoint at the revision before them. With `branch`, the
    /// batch runs in that branch session instead (`collab`).
    pub(crate) fn apply(&mut self, sid: &str, p: ApplyParams) -> R {
        if let Some(branch) = p.branch.clone() {
            return crate::collab::branch_apply(self, sid, &branch, &p);
        }
        let session = self
            .sessions
            .live(sid)
            .cloned()
            .ok_or_else(|| vec![session_not_found(sid)])?;
        let (doc, mut batch) = self.batch(&p)?;
        let dry_run = batch.dry_run;
        let missing_why = p.why.as_deref().is_none_or(|w| w.trim().is_empty());
        if matches!(session.author, Author::Agent(_)) && !dry_run && missing_why {
            // Report every problem at once: what a dry run finds, then the `why`.
            batch.dry_run = true;
            let mut errors =
                match self
                    .registry
                    .apply(&mut self.store, &batch, &session.cmd_session())
                {
                    Ok(_) => Vec::new(),
                    Err(d) => relocate(d),
                };
            errors.push(
                Diagnostic::error(
                    codes::MISSING_WHY,
                    "agent changes need a `why`: one sentence on the reason, shown to the human who reviews them",
                )
                .path("why")
                .with_fix(
                    "Add a `why`",
                    FixAction::Set {
                        record: None,
                        path: "why".into(),
                        value: "<one sentence: why this change>".into(),
                    },
                ),
            );
            return Err(errors);
        }
        let before = self.store.revision();
        let result = match self
            .gesture_apply(sid, &batch, &session.cmd_session())
            .map_err(relocate)?
        {
            Some(result) => result,
            None => self
                .registry
                .apply(&mut self.store, &batch, &session.cmd_session())
                .map_err(relocate)?,
        };
        let turn = matches!(session.author, Author::Agent(_))
            && !dry_run
            && !result.replayed
            && (result.txn.is_some() || self.held_count(sid) > 0);
        let checkpoint = turn.then(|| checkpoint_at(self, sid, before, p.why.clone(), true).id);
        let mut text = apply_text(self, &doc, &result, checkpoint.as_deref());
        let held = self.held_count(sid);
        if held > 0 {
            text.push_str(&format!("{held} field operation(s) wait for the active human gesture (at most two seconds).\n"));
        }
        Ok(json!({
            "ok": true,
            "doc": path_of(self, &doc),
            "text": text,
            "dryRun": result.dry_run,
            "txn": result.txn,
            "replayed": result.replayed,
            "revision": result.revision,
            "ids": result.symbols,
            "diff": diff_summary(&result.ops),
            "outputs": result.outputs,
            "ops": result.ops,
            "checkpoint": checkpoint,
            "gestureHeld": held,
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
