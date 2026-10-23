//! The fallback: everything the bus says about a subject no other explainer covers
//! (an entity asked about something other than visibility, a system, a pass), with
//! causal chains and fixes.

use ara_diag::store::Filter;
use ara_diag::{Diagnostic, Severity, Subject as Thing};

use super::{line, runnable};
use crate::{Cause, Collector, Explainer, Facts, Findings, Likelihood, Question, Report, Status};

/// Explains a subject from the diagnostics about it.
pub struct Subject;

impl Explainer for Subject {
    fn name(&self) -> &'static str {
        "subject"
    }

    fn about(&self) -> &'static str {
        "Everything the diagnostics bus says about an entity, asset, system, pass or module, with causes and fixes."
    }

    fn examples(&self) -> &'static [&'static str] {
        &["name:Door not opening", "system:physics.step"]
    }

    fn matches(&self, q: &Question) -> u32 {
        if q.entities.is_empty() && q.subjects.is_empty() {
            0
        } else {
            30
        }
    }

    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        _extra: &[&dyn Collector],
    ) -> Result<Report, Vec<Diagnostic>> {
        let mut subjects = q.subjects.clone();
        let mut label = q.subjects.first().map(ToString::to_string);
        if let Some(sel) = q.entities.first() {
            let e = facts.entity(sel, q.doc.as_deref())?;
            label = Some(format!("{} in {}", e.label(), e.doc));
            subjects.push(Thing::entity(e.address));
        }
        let records = facts.diagnostics(&Filter {
            subjects,
            ..Filter::default()
        });
        let mut f = Findings::default();
        f.check(
            "on the bus",
            Status::Ok,
            format!("{} records", records.len()),
        );
        for r in records
            .iter()
            .rev()
            .filter(|r| r.diagnostic.severity != Severity::Info)
            .take(5)
        {
            let likelihood = if r.diagnostic.is_error() {
                Likelihood::High
            } else {
                Likelihood::Medium
            };
            let mut c = Cause::new(r.diagnostic.code, likelihood, r.diagnostic.message.clone())
                .evidence(line(r));
            for parent in facts.causes(r) {
                c = c.evidence(format!("caused by: {}", line(&parent)));
            }
            c.fixes = runnable(&r.diagnostic.fixes);
            f.cause(c);
        }
        for r in records
            .iter()
            .rev()
            .filter(|r| r.diagnostic.severity == Severity::Info)
            .take(3)
        {
            f.related.push(line(r));
        }
        let topic = q.words.join(" ");
        let answer = if f.causes.is_empty() {
            let known = "visibility, reload, frame, code, asset";
            format!(
                "nothing on the bus explains it{}; explainers exist for: {known}",
                if topic.is_empty() {
                    String::new()
                } else {
                    format!(" (no explainer covers `{topic}` yet)")
                }
            )
        } else {
            Report::verdict(&f, "")
        };
        Ok(Report::new(&q.text, "subject", label, answer, f))
    }
}
