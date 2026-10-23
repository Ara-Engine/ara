//! The `diag` service: the diagnostics bus on the Engine Protocol, and the `logs`
//! method behind `ara_logs`.
//!
//! - `diag.query`: records filtered by code, severity, subject, origin, time, tick
//!   and text, with a grouped summary (`group`, default on) within a character
//!   `budget`. `logs` is the same query with the agent-friendly subset of params.
//! - `diag.subscribe` / `diag.unsubscribe`: stream matching records to this
//!   connection as `diag.event` notifications (`{subscription, record}`), whether
//!   they were logged by the host or drained from the bus.
//! - `diag.clear`: forget matching records.
//! - `diag.report`: other processes (players, import workers) push diagnostics into
//!   this host's bus.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_diag::bus::{Meta, Origin};
use ara_diag::store::{Filter, OriginFilter, Record, Saved, Store as DiagStore};
use ara_diag::summary::summarize;
use ara_diag::{Code, Diagnostic, Severity, Subject, catalog, codes, did_you_mean};
use ara_proto::{ConnId, Scope};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::host::State;
use crate::params::{LogsParams, SeverityParam};
use crate::service::{Cx, Method, Service, parse};

type R = Result<Value, Vec<Diagnostic>>;

/// Default text budget of a grouped answer, in characters (about 500 tokens).
const BUDGET: u32 = 2000;

/// Params of `diag.query`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagQueryParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only records new or repeated after this cursor (`cursor` of a previous call).
    pub since: Option<u64>,
    /// Only these codes (`ARA1302`, `ARA-EXP-AND-007`).
    pub codes: Option<Vec<String>>,
    /// Minimum severity (default `info`).
    pub severity: Option<SeverityParam>,
    /// Only records about these subjects: `entity:e_8b21`, `name`-free IDs
    /// (`e_8b21`), `asset:@meshes/crate.glb`, `system:physics.step`, `pass:gbuffer`,
    /// `module:Gameplay`.
    pub subjects: Option<Vec<String>>,
    /// Only records caused by `engine`, `human`, `agent` or `agent:<name>`.
    pub origin: Option<String>,
    /// Only records last seen at or after this Unix time in milliseconds.
    pub from_ms: Option<u64>,
    /// Only records last seen at or before this Unix time in milliseconds.
    pub to_ms: Option<u64>,
    /// Only records last seen at or after this simulation tick.
    pub from_tick: Option<u64>,
    /// Only records whose message contains this text.
    pub text: Option<String>,
    /// Group records by code with counts, subjects and fixes (default true).
    pub group: Option<bool>,
    /// Approximate size of the text answer in characters (default 2000).
    pub budget: Option<u32>,
    /// Maximum records in `entries`, newest kept (default 10 grouped, 50 not).
    pub limit: Option<u32>,
}

/// Params of `diag.subscribe`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagSubscribeParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only these codes.
    pub codes: Option<Vec<String>>,
    /// Minimum severity.
    pub severity: Option<SeverityParam>,
    /// Only records about these subjects.
    pub subjects: Option<Vec<String>>,
    /// Only records caused by `engine`, `human`, `agent` or `agent:<name>`.
    pub origin: Option<String>,
}

/// Params of `diag.unsubscribe`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagUnsubscribeParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// The `subscription` returned by `diag.subscribe`.
    pub subscription: String,
}

/// Params of `diag.clear`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagClearParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only these codes (default: all).
    pub codes: Option<Vec<String>>,
    /// Only records at or above this severity.
    pub severity: Option<SeverityParam>,
    /// Only records about these subjects.
    pub subjects: Option<Vec<String>>,
    /// Only records caused by `engine`, `human`, `agent` or `agent:<name>`.
    pub origin: Option<String>,
}

/// Params of `diag.report`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagReportParams {
    /// Session handle from `session.begin`; its author is the diagnostics' origin
    /// unless the reporter is the engine (`engine: true`).
    pub session: String,
    /// Diagnostics as JSON (`{code, severity, message, location?, subjects?, fixes?,
    /// data?}`), each optionally with `causedBy`: the index of an earlier diagnostic
    /// in this batch that caused it.
    pub diagnostics: Vec<Value>,
    /// Simulation tick they happened on.
    pub tick: Option<u64>,
    /// The reporter is an engine process (player, import worker), not the session's
    /// author.
    pub engine: Option<bool>,
}

/// One live `diag.subscribe`.
#[derive(Debug, Clone)]
pub(crate) struct Subscription {
    /// The connection notifications go to.
    pub(crate) conn: ConnId,
    /// What it wants.
    pub(crate) filter: Filter,
}

/// Subscriptions by ID.
pub(crate) type Subscriptions = BTreeMap<String, Subscription>;

fn severity(p: Option<SeverityParam>) -> Option<Severity> {
    p.map(|s| match s {
        SeverityParam::Error => Severity::Error,
        SeverityParam::Warning => Severity::Warning,
        SeverityParam::Info => Severity::Info,
    })
}

/// `ARA2011` for an unknown code, with the closest known code as a fix.
fn unknown_code(text: &str, index: usize) -> Diagnostic {
    let names: Vec<String> = catalog::all().iter().map(|d| d.code.to_string()).collect();
    let mut d = Diagnostic::error(
        codes::UNKNOWN_CODE,
        format!(
            "`{text}` is not a diagnostic code; codes look like `ARA1302` or `ARA-EXP-AND-007`"
        ),
    )
    .path(format!("params.codes[#{index}]"));
    if let Some(near) = did_you_mean(text, names.iter().map(String::as_str)) {
        d.message.push_str(&format!("; did you mean `{near}`?"));
    }
    d
}

/// A store filter from protocol params; every bad part is reported at once.
fn filter(
    codes_in: Option<Vec<String>>,
    severity_in: Option<SeverityParam>,
    subjects_in: Option<Vec<String>>,
    origin_in: Option<String>,
) -> Result<Filter, Vec<Diagnostic>> {
    let mut problems = Vec::new();
    let mut f = Filter {
        severity: severity(severity_in),
        ..Filter::default()
    };
    for (i, c) in codes_in.unwrap_or_default().iter().enumerate() {
        match Code::parse(c) {
            Some(code) => f.codes.push(code),
            None => problems.push(unknown_code(c, i)),
        }
    }
    for (i, s) in subjects_in.unwrap_or_default().iter().enumerate() {
        match s.parse::<Subject>() {
            Ok(subject) => f.subjects.push(subject),
            Err(e) => problems.push(
                Diagnostic::error(codes::INVALID_PARAMS, e).path(format!("params.subjects[#{i}]")),
            ),
        }
    }
    if let Some(o) = origin_in {
        match OriginFilter::parse(&o) {
            Some(of) => f.origin = Some(of),
            None => problems.push(
                Diagnostic::error(
                    codes::INVALID_PARAMS,
                    format!(
                        "`{o}` is not an origin; use `engine`, `human`, `agent` or `agent:<name>`"
                    ),
                )
                .path("params.origin"),
            ),
        }
    }
    if problems.is_empty() {
        Ok(f)
    } else {
        Err(problems)
    }
}

/// What a query answers: records, a grouped summary and its text.
fn answer(st: &State, f: &Filter, group: bool, budget: u32, limit: Option<u32>) -> Value {
    let all = st.diag.query(&Filter {
        limit: 0,
        ..f.clone()
    });
    let cursor = st.diag.cursor();
    let limit = limit.map_or(if group { 10 } else { 50 }, |l| l.clamp(1, 1000)) as usize;
    let entries: Vec<&Record> = all[all.len().saturating_sub(limit)..].to_vec();
    if all.is_empty() {
        let text = if f.since > 0 {
            format!("no new diagnostics since #{} (cursor {cursor})\n", f.since)
        } else {
            format!("no diagnostics (cursor {cursor})\n")
        };
        return json!({ "cursor": cursor, "total": 0, "entries": [], "text": text });
    }
    if !group {
        let mut text = String::new();
        for r in &entries {
            let count = if r.count > 1 {
                format!(" ×{}", r.count)
            } else {
                String::new()
            };
            text.push_str(&format!("#{}{count} {}\n", r.seq, r.diagnostic));
        }
        if all.len() > entries.len() {
            text.push_str(&format!(
                "… {} older (raise `limit`)\n",
                all.len() - entries.len()
            ));
        }
        text.push_str(&format!("cursor {cursor}\n"));
        return json!({ "cursor": cursor, "total": all.len(), "entries": entries, "text": text });
    }
    let summary = summarize(&all, 3);
    let mut text = summary.text(budget.max(200) as usize);
    let worst = summary
        .groups
        .iter()
        .find(|g| g.severity == Severity::Error)
        .map(|g| g.code);
    match worst {
        Some(code) => text.push_str(&format!("cursor {cursor} · explain: ara why {code}\n")),
        None => text.push_str(&format!("cursor {cursor}\n")),
    }
    json!({
        "cursor": cursor,
        "total": all.len(),
        "summary": summary,
        "entries": entries,
        "text": text,
    })
}

/// `logs` (`ara_logs`): the agent-friendly query.
pub(crate) fn logs(st: &mut State, p: LogsParams) -> R {
    let mut subjects = p.subjects;
    if let Some(e) = p.entity {
        subjects
            .get_or_insert_with(Vec::new)
            .push(format!("entity:{e}"));
    }
    let mut f = filter(p.codes, p.severity, subjects, p.origin)?;
    f.since = p.since.unwrap_or(0);
    f.text = p.text;
    Ok(answer(
        st,
        &f,
        p.group.unwrap_or(true),
        p.budget.unwrap_or(BUDGET),
        p.limit,
    ))
}

fn query(st: &mut State, p: DiagQueryParams) -> R {
    let mut f = filter(p.codes, p.severity, p.subjects, p.origin)?;
    f.since = p.since.unwrap_or(0);
    f.from_ms = p.from_ms;
    f.to_ms = p.to_ms;
    f.from_tick = p.from_tick;
    f.text = p.text;
    Ok(answer(
        st,
        &f,
        p.group.unwrap_or(true),
        p.budget.unwrap_or(BUDGET),
        p.limit,
    ))
}

fn report(st: &mut State, origin: Origin, p: DiagReportParams) -> R {
    let origin = if p.engine.unwrap_or(false) {
        Origin::Engine
    } else {
        origin
    };
    let mut recorded = Vec::new();
    let mut skipped = Vec::new();
    let mut ids = Vec::new();
    for (i, mut v) in p.diagnostics.into_iter().enumerate() {
        let cause = v
            .as_object_mut()
            .and_then(|o| o.remove("causedBy"))
            .and_then(|c| c.as_u64())
            .and_then(|c| ids.get(c as usize).copied().flatten());
        match serde_json::from_value::<Diagnostic>(v) {
            Ok(d) => {
                let meta = Meta {
                    origin: Some(origin.clone()),
                    tick: p.tick,
                    cause,
                    ..Meta::default()
                };
                let r = st.diag.push(d, meta);
                ids.push(Some(r.id));
                recorded.push(r.seq);
            }
            Err(e) => {
                ids.push(None);
                skipped.push(
                    Diagnostic::warning(
                        codes::DIAG_INVALID_REPORT,
                        format!("reported diagnostic #{i} is not a diagnostic: {e}"),
                    )
                    .path(format!("params.diagnostics[#{i}]")),
                )
            }
        }
    }
    for d in &skipped {
        recorded.push(st.diag.push(d.clone(), Meta::default()).seq);
    }
    st.keep(&recorded);
    st.published(&recorded);
    Ok(json!({
        "recorded": recorded.len() - skipped.len(),
        "skipped": skipped,
        "cursor": st.diag.cursor(),
    }))
}

/// The `diag` service.
pub(crate) struct DiagService;

impl Service for DiagService {
    fn name(&self) -> &'static str {
        "diag"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<DiagQueryParams>(db, "diag.query", "Diagnostics on the bus by code, severity, subject, origin, time or tick; grouped and budgeted by default.")
                .read_only(),
            Method::new::<DiagSubscribeParams>(db, "diag.subscribe", "Stream matching diagnostics to this connection as `diag.event` notifications."),
            Method::new::<DiagUnsubscribeParams>(db, "diag.unsubscribe", "Stop a `diag.subscribe` stream."),
            Method::new::<DiagClearParams>(db, "diag.clear", "Forget matching diagnostics (all without filters).")
                .destructive(),
            Method::new::<DiagReportParams>(db, "diag.report", "Push diagnostics from another process (player, import worker) onto this host's bus.")
                .scope(Scope::Read),
        ]
    }

    fn call(&mut self, cx: &mut Cx<'_>, method: &str, params: Value) -> R {
        let origin = cx
            .session()
            .map_or(Origin::Engine, |s| Origin::of_author(&s.author.to_string()));
        let conn = cx.conn();
        let st = &mut *cx.st;
        match method {
            "diag.query" => query(st, parse(params)?),
            "diag.subscribe" => {
                let p: DiagSubscribeParams = parse(params)?;
                let _ = p.session;
                let filter = filter(p.codes, p.severity, p.subjects, p.origin)?;
                let id = st.token("dsub");
                st.diag_subs
                    .insert(id.clone(), Subscription { conn, filter });
                Ok(json!({ "subscription": id, "cursor": st.diag.cursor() }))
            }
            "diag.unsubscribe" => {
                let p: DiagUnsubscribeParams = parse(params)?;
                match st.diag_subs.get(&p.subscription) {
                    Some(s) if s.conn == conn => {
                        st.diag_subs.remove(&p.subscription);
                        Ok(json!({ "unsubscribed": p.subscription }))
                    }
                    _ => Err(vec![
                        Diagnostic::error(
                            codes::INVALID_PARAMS,
                            format!("no subscription `{}` on this connection", p.subscription),
                        )
                        .path("params.subscription"),
                    ]),
                }
            }
            "diag.clear" => {
                let p: DiagClearParams = parse(params)?;
                let f = filter(p.codes, p.severity, p.subjects, p.origin)?;
                let removed = st.diag.clear(&f);
                if removed > 0
                    && let Err(e) = st.diag_file.rewrite(&st.diag)
                {
                    st.log(Diagnostic::warning(
                        codes::IO,
                        format!("cannot rewrite `.ara/diagnostics.jsonl`: {e}"),
                    ));
                }
                Ok(json!({ "removed": removed, "cursor": st.diag.cursor() }))
            }
            "diag.report" => report(st, origin, parse(params)?),
            other => Err(vec![Diagnostic::error(
                codes::UNKNOWN_METHOD,
                format!("diag does not handle `{other}`"),
            )]),
        }
    }
}

/// The engine's runtime diagnostics, kept in `.ara/diagnostics.jsonl` so `ara why`
/// still sees what a play process or an import worker reported after the host
/// restarts. Call errors belong to their session and are not kept. Lines are
/// appended as records change; the newest line per record wins, and the file is
/// rewritten once it holds four times more lines than records.
pub(crate) struct Persist {
    path: PathBuf,
    dirty: BTreeSet<u64>,
    lines: usize,
    last: Instant,
}

/// How often changed records are appended.
const PERSIST_EVERY: Duration = Duration::from_millis(500);

impl Persist {
    /// Loads `.ara/diagnostics.jsonl` into `store`; returns the file and how many
    /// lines could not be read.
    pub(crate) fn open(state_dir: &Path, store: &mut DiagStore) -> (Persist, usize) {
        let path = state_dir.join("diagnostics.jsonl");
        let text = fs::read_to_string(&path).unwrap_or_default();
        let mut saved = Vec::new();
        let mut bad = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Saved>(line) {
                Ok(s) => saved.push(s),
                Err(_) => bad += 1,
            }
        }
        let lines = saved.len();
        store.restore(saved);
        let persist = Persist {
            path,
            dirty: BTreeSet::new(),
            lines,
            last: Instant::now(),
        };
        (persist, bad)
    }

    /// Marks a record as changed if it is the engine's.
    pub(crate) fn mark(&mut self, record: &Record) {
        if record.origin == Origin::Engine {
            self.dirty.insert(record.seq);
        }
    }

    /// Appends changed records (at most every `PERSIST_EVERY` unless `now`), and
    /// compacts the file when it has grown.
    pub(crate) fn flush(&mut self, store: &DiagStore, now: bool) -> std::io::Result<()> {
        if self.dirty.is_empty() || (!now && self.last.elapsed() < PERSIST_EVERY) {
            return Ok(());
        }
        self.last = Instant::now();
        if self.lines + self.dirty.len() > 4 * store.len() + 256 {
            return self.rewrite(store);
        }
        let mut text = String::new();
        let mut lines = 0;
        for seq in &self.dirty {
            if let Some(saved) = store.save(*seq) {
                text.push_str(&serde_json::to_string(&saved).unwrap_or_default());
                text.push('\n');
                lines += 1;
            }
        }
        // On failure the records stay dirty and the next flush retries them.
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(text.as_bytes())?;
        self.dirty.clear();
        self.lines += lines;
        Ok(())
    }

    /// Writes the engine's current records only (after a clear, or to compact).
    pub(crate) fn rewrite(&mut self, store: &DiagStore) -> std::io::Result<()> {
        let mut text = String::new();
        let mut lines = 0;
        for r in store.query(&Filter::default()) {
            if r.origin == Origin::Engine
                && let Some(saved) = store.save(r.seq)
            {
                text.push_str(&serde_json::to_string(&saved).unwrap_or_default());
                text.push('\n');
                lines += 1;
            }
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &self.path)?;
        self.dirty.clear();
        self.lines = lines;
        Ok(())
    }
}
