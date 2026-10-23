//! "Why did reload fall back to L3?" — reads the C# reload reports `ara-script`
//! puts on the bus (`ARA2020` applied, `ARA2021` escalated, compile and load
//! errors) and explains each escalation reason with what to do about it.
//!
//! An escalation's `data.reason` mirrors the `ESC_*` constants of the reload ABI
//! (`closure`, `structural`, `cold`, ...).

use std::collections::BTreeMap;

use ara_diag::store::{Filter, Record};
use ara_diag::{Code, Diagnostic, Subject, SubjectKind, codes};
use serde_json::Value;

use super::line;
use crate::{
    Cause, Collector, Explainer, Facts, Findings, Likelihood, Question, Report, Status, Target,
};

/// `script::COMPILE_ERROR`: the edit did not compile.
const COMPILE_ERROR: Code = Code(900);
/// `script::COMPILE_SERVER`: the compile server failed.
const COMPILE_SERVER: Code = Code(904);
/// `script::DELTA_REJECTED`: the runtime rejected an EnC delta.
const DELTA_REJECTED: Code = Code(905);
/// `script::MODULE_LARGE` and `MODULE_TOO_LARGE`: L3 cost grows with module size.
const MODULE_LARGE: [Code; 2] = [Code(902), Code(903)];
/// `script::DEPENDENCY_FAILED`, `RESTART_REQUIRED`, `GENERATION_LEAKED`.
const CASCADE: [Code; 3] = [Code(925), Code(926), Code(927)];

const WORDS: &[&str] = &[
    "reload", "l1", "l2", "l3", "l4", "hot", "escalat", "enc", "fallback", "fell", "swap",
];

/// What an escalation reason means and what to do about it.
fn reason(key: &str) -> (&'static str, &'static str) {
    match key {
        "closure" => (
            "a changed method contains a lambda, local function or LINQ query (EnC deltas for closures are not safe)",
            "move the lambda's body into a named method; edits to that method then take L1",
        ),
        "structural" => (
            "a declaration changed: a signature, struct field, attribute or virtual member",
            "batch declaration changes; body-only edits take L1",
        ),
        "state_machine" => (
            "a changed method is an iterator or `async` (a state machine)",
            "keep the changing logic in a plain helper the iterator or async method calls",
        ),
        "constructor" => (
            "a constructor or field initializer changed",
            "set initial values in `OnSpawn` while iterating instead of constructors or initializers",
        ),
        "cold" => (
            "the module was cold (optimized IL); its first edit makes it hot",
            "expected once per module after a cold start; later body edits take L1",
        ),
        "debugger" => (
            "a debugger is attached, and the runtime refuses EnC deltas then",
            "detach the debugger while iterating",
        ),
        "file_set" => (
            "a file was added, removed or renamed",
            "expected; later body edits take L1",
        ),
        "generated" => (
            "generated code changed (attributes, partial properties, registration)",
            "expected when declarations change; body-only edits take L1",
        ),
        "emit_failed" => (
            "Roslyn could not emit a delta for this edit",
            "check the compile server output; it is a compiler limit for this kind of edit",
        ),
        "requested" => (
            "the host asked for a full image (after a load failure or a heat change)",
            "see the load error before it",
        ),
        "runtime_rejected" => (
            "the runtime rejected the EnC delta (ARA0905)",
            "report it if it repeats for plain body edits",
        ),
        "capability" => (
            "the runtime lacks the EnC capability this additive edit needs",
            "split the edit: add the new members first, use them in a second edit",
        ),
        "dependency" => (
            "a module it references changed its public surface, so it was rebuilt against it",
            "keep public surfaces stable while iterating on bodies",
        ),
        "relink" => (
            "a module it references got a new generation, so it was reloaded against it",
            "expected after the dependency's own L3",
        ),
        _ => (
            "an escalation the explainer does not know",
            "read the message",
        ),
    }
}

/// The escalation reason of a record: `data.reason`, else a trailing `(reason)`.
fn reason_of(r: &Record) -> String {
    if let Some(Value::String(s)) = r.diagnostic.data.as_deref().and_then(|d| d.get("reason")) {
        return s.clone();
    }
    let m = &r.diagnostic.message;
    m.rsplit_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or("unknown")
        .to_owned()
}

/// Explains C# reload levels.
pub struct Reload;

impl Explainer for Reload {
    fn name(&self) -> &'static str {
        "reload"
    }

    fn about(&self) -> &'static str {
        "Why a C# edit took a full module swap (L3) or did not reload, from the reload reports on the bus."
    }

    fn examples(&self) -> &'static [&'static str] {
        &["did reload fall back to L3", "module:Gameplay reload slow"]
    }

    fn matches(&self, q: &Question) -> u32 {
        if q.about(WORDS) {
            90
        } else if q.subject(SubjectKind::Module).is_some() && q.entities.is_empty() {
            60
        } else {
            0
        }
    }

    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        extra: &[&dyn Collector],
    ) -> Result<Report, Vec<Diagnostic>> {
        let module = q.subject(SubjectKind::Module).map(|s| s.value.clone());
        let subjects: Vec<Subject> = module.iter().map(|m| Subject::module(m.clone())).collect();
        let records = facts.diagnostics(&Filter {
            codes: [
                vec![
                    codes::RELOAD_APPLIED,
                    codes::RELOAD_ESCALATED,
                    COMPILE_ERROR,
                    COMPILE_SERVER,
                    DELTA_REJECTED,
                ],
                MODULE_LARGE.to_vec(),
                CASCADE.to_vec(),
            ]
            .concat(),
            subjects,
            ..Filter::default()
        });
        let mut f = Findings::default();
        let applied: u32 = records
            .iter()
            .filter(|r| r.diagnostic.code == codes::RELOAD_APPLIED)
            .map(|r| r.count)
            .sum();
        let escalations: Vec<&Record> = records
            .iter()
            .filter(|r| r.diagnostic.code == codes::RELOAD_ESCALATED)
            .collect();
        f.check(
            "reload reports",
            if records.is_empty() {
                Status::Unknown
            } else {
                Status::Ok
            },
            format!(
                "{applied} applied, {} escalated",
                escalations.iter().map(|r| r.count).sum::<u32>()
            ),
        );
        // One cause per reason; the most recent reason is the likely answer.
        let mut by_reason: BTreeMap<String, (u32, u64, Vec<&Record>)> = BTreeMap::new();
        for r in &escalations {
            let e = by_reason.entry(reason_of(r)).or_default();
            e.0 += r.count;
            e.1 = e.1.max(r.seq);
            e.2.push(r);
        }
        let latest = by_reason
            .iter()
            .max_by_key(|(_, v)| v.1)
            .map(|(k, _)| k.clone());
        // Newest reason first.
        let mut reasons: Vec<_> = by_reason.into_iter().collect();
        reasons.sort_by_key(|(_, (_, newest, _))| std::cmp::Reverse(*newest));
        for (key, (count, _, rs)) in reasons {
            let (what, todo) = reason(&key);
            let likelihood = if Some(&key) == latest.as_ref() {
                Likelihood::High
            } else {
                Likelihood::Medium
            };
            let mut c = Cause::new(
                codes::RELOAD_ESCALATED,
                likelihood,
                format!("L3 because {what} ({key}, {count}×)"),
            )
            .advice(todo);
            for r in rs.iter().rev().take(2) {
                c = c.evidence(line(r));
                for s in &r.diagnostic.subjects {
                    c = c.subject(s.clone());
                }
            }
            f.cause(c);
        }
        for r in records
            .iter()
            .filter(|r| r.diagnostic.code == COMPILE_ERROR || r.diagnostic.code == COMPILE_SERVER)
        {
            // A failure is current only if no reload of the same module came after it.
            let module = |x: &Record| {
                x.diagnostic
                    .subjects
                    .iter()
                    .find(|s| s.kind == SubjectKind::Module)
                    .cloned()
            };
            let fixed = records.iter().any(|later| {
                later.seq > r.seq
                    && matches!(later.diagnostic.code, c if c == codes::RELOAD_APPLIED || c == codes::RELOAD_ESCALATED)
                    && module(later) == module(r)
            });
            let cause = if fixed {
                Cause::new(
                    r.diagnostic.code,
                    Likelihood::Low,
                    "An earlier edit failed to compile; a later reload of the module succeeded"
                        .to_owned(),
                )
            } else {
                Cause::new(
                    r.diagnostic.code,
                    Likelihood::High,
                    "The last edit did not reload at all: it failed to compile, so the last good code keeps running".to_owned(),
                )
            };
            f.cause(cause.evidence(line(r)));
        }
        for r in records.iter().filter(|r| {
            MODULE_LARGE.contains(&r.diagnostic.code)
                || CASCADE.contains(&r.diagnostic.code)
                || r.diagnostic.code == DELTA_REJECTED
        }) {
            f.cause(
                Cause::new(
                    r.diagnostic.code,
                    Likelihood::Medium,
                    r.diagnostic.message.clone(),
                )
                .evidence(line(r)),
            );
        }
        f.collect(extra, &Target::Module(module.as_deref()), facts);
        let answer = if records.is_empty() {
            "no C# reload has been reported to this host; reports arrive from the editor or play process when a module reloads".to_owned()
        } else {
            Report::verdict(
                &f,
                "every reload was in place (L1/L2); none fell back to L3",
            )
        };
        let subject = module.map(|m| format!("module `{m}`"));
        Ok(Report::new(&q.text, "reload", subject, answer, f))
    }
}
