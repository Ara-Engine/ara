//! `ara why ARA1302`: what a code means, where it is happening now, and how to fix
//! each occurrence.
//!
//! The doc comes from the catalog (title, explanation, causes, fixes). Instances
//! come from the bus, grouped by subject, newest first, each with its own fixes and
//! its causal chain; with none on the bus, the catalog's causes stand in.

use std::collections::BTreeMap;

use ara_diag::store::Filter;
use ara_diag::{Diagnostic, catalog, codes};

use super::{line, runnable};
use crate::{Cause, Collector, Explainer, Facts, Findings, Likelihood, Question, Report, Status, Target};

/// Instances turned into causes.
const MAX_INSTANCES: usize = 5;

/// Explains diagnostic codes.
pub struct CodeExplainer;

impl Explainer for CodeExplainer {
    fn name(&self) -> &'static str {
        "code"
    }

    fn about(&self) -> &'static str {
        "What a diagnostic code means, where it happens now, and the fix for each occurrence."
    }

    fn examples(&self) -> &'static [&'static str] {
        &["ARA1302", "ARA-EXP-AND-007"]
    }

    fn matches(&self, q: &Question) -> u32 {
        if q.codes.is_empty() { 0 } else { 80 }
    }

    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        extra: &[&dyn Collector],
    ) -> Result<Report, Vec<Diagnostic>> {
        let code = q.codes[0];
        let Some(doc) = catalog::get(code) else {
            return Err(vec![Diagnostic::error(
                codes::UNKNOWN_CODE,
                format!("`{code}` has no documentation in this engine version"),
            )]);
        };
        let mut f = Findings::default();
        let mut subjects = q.subjects.clone();
        subjects.extend(q.entities.iter().filter_map(|e| ara_diag::Subject::parse(e)));
        let records = facts.diagnostics(&Filter {
            codes: vec![code],
            subjects,
            ..Filter::default()
        });
        let total: u64 = records.iter().map(|r| u64::from(r.count)).sum();
        f.check(
            "on the bus",
            if records.is_empty() { Status::Skip } else { Status::Fail },
            if records.is_empty() {
                "no occurrences now".to_owned()
            } else {
                format!("{} records, {total} occurrences", records.len())
            },
        );
        // Newest first, one cause per distinct location.
        let mut seen: BTreeMap<String, ()> = BTreeMap::new();
        for r in records.iter().rev() {
            let where_ = r.diagnostic.location.to_string();
            if seen.insert(where_.clone(), ()).is_some() {
                continue;
            }
            if f.causes.len() >= MAX_INSTANCES {
                break;
            }
            let d = &r.diagnostic;
            let mut cause = Cause::new(code, Likelihood::High, d.message.clone());
            if !where_.is_empty() {
                cause = cause.evidence(format!("at {where_}"));
            }
            if r.count > 1 {
                cause = cause.evidence(format!("seen {} times (#{})", r.count, r.seq));
            }
            for parent in facts.causes(r) {
                cause = cause.evidence(format!("caused by: {}", line(&parent)));
            }
            for s in r.subjects() {
                cause = cause.subject(s);
            }
            cause.fixes = runnable(&d.fixes);
            if cause.fixes.is_empty() {
                cause.advice = doc.fixes.iter().take(2).map(|s| (*s).to_owned()).collect();
            }
            f.cause(cause);
        }
        let shown = f.causes.len();
        if records.len() > shown && shown > 0 {
            f.related.push(format!(
                "{} more {code} records: ara_logs {{codes: [\"{code}\"], group: false}}",
                records.len() - shown
            ));
        }
        if records.is_empty() {
            for c in doc.causes.iter().take(3) {
                f.cause(Cause::new(code, Likelihood::Low, *c));
            }
        }
        f.collect(extra, &Target::Code(code), facts);
        let answer = if records.is_empty() {
            format!("{code} {}: {} Not on the bus now; its usual causes:", doc.title, doc.summary())
        } else {
            format!("{code} {}: {} {total} occurrences now:", doc.title, doc.summary())
        };
        let mut report = Report::new(&q.text, "code", None, answer, f);
        let mut page = String::from("Fixes in general:\n");
        for fix in &doc.fixes {
            page.push_str(&format!("- {fix}\n"));
        }
        page.push_str(&format!("Docs: ara://docs/errors/{code} (ara_docs {code})"));
        report.doc = Some(page);
        Ok(report)
    }
}
