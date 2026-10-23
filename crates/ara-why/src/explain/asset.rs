//! `ara why @meshes/crate.glb` / `@materials/crate.material`: does the asset
//! resolve, is its source there, did it import, does a material compile, and who
//! uses it.

use ara_diag::store::Filter;
use ara_diag::{Diagnostic, Severity, Subject, SubjectKind, codes};

use super::{line, material};
use crate::{
    Cause, Collector, Explainer, Facts, Findings, Likelihood, Question, Report, Status, Target,
};

/// Explains assets.
pub struct AssetExplainer;

impl Explainer for AssetExplainer {
    fn name(&self) -> &'static str {
        "asset"
    }

    fn about(&self) -> &'static str {
        "An asset's state: does it resolve, is its source there, did it import, does a material compile, who uses it."
    }

    fn examples(&self) -> &'static [&'static str] {
        &["@materials/crate.material", "@meshes/crate.glb broken"]
    }

    fn matches(&self, q: &Question) -> u32 {
        let asset = q
            .subjects
            .iter()
            .any(|s| matches!(s.kind, SubjectKind::Asset | SubjectKind::Document));
        if asset && q.entities.is_empty() && q.codes.is_empty() {
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
        let subject = q
            .subjects
            .iter()
            .find(|s| matches!(s.kind, SubjectKind::Asset | SubjectKind::Document))
            .cloned()
            .ok_or_else(|| {
                vec![Diagnostic::error(
                    codes::WHY_UNANSWERED,
                    "name an asset: `@meshes/crate.glb`",
                )]
            })?;
        let asset = facts.asset(&subject.value);
        let mut f = Findings::default();
        if !asset.resolves() {
            f.check("resolves", Status::Fail, "");
            let mut c = Cause::new(
                codes::UNRESOLVED_REF,
                Likelihood::High,
                format!(
                    "`{}` does not resolve: no asset has that ID or path",
                    asset.reference
                ),
            );
            if let Some(d) = &asset.problem {
                c = c.evidence(format!("{} {}", d.code, d.message));
            }
            for near in &asset.closest {
                c = c.advice(format!("did you mean `{near}`?"));
            }
            f.cause(c);
            let report = Report::new(
                &q.text,
                "asset",
                Some(asset.reference.clone()),
                "it does not exist",
                f,
            );
            return Ok(report);
        }
        let canonical = asset.canonical();
        f.check("resolves", Status::Ok, canonical.clone());
        let about = Subject::asset(canonical.clone());
        match asset.source_exists {
            Some(false) => {
                f.check("source", Status::Fail, "missing");
                f.cause(
                    Cause::new(
                        codes::ASSET_SOURCE_MISSING,
                        Likelihood::Medium,
                        format!(
                            "Its source `{}` is missing; the sidecar keeps the ID, but nothing can be imported",
                            asset.path.as_deref().unwrap_or_default()
                        ),
                    )
                    .advice("restore the file next to its `.import.json` (`git lfs pull`), or move the sidecar with a renamed source"),
                );
            }
            Some(true) => f.check("source", Status::Ok, ""),
            None => {}
        }
        if let Some(status) = material::inspect(facts, &asset) {
            if status.errors.is_empty() {
                f.check(
                    "compiles",
                    Status::Ok,
                    status.domain.map_or("", |d| d.name()),
                );
            } else {
                f.check(
                    "compiles",
                    Status::Fail,
                    format!("{} errors", status.errors.len()),
                );
                for d in status.errors.iter().take(5) {
                    let mut c = Cause::new(d.code, Likelihood::High, d.message.clone())
                        .evidence(format!("at {}", d.location))
                        .subject(about.clone());
                    c.fixes = d.fixes.clone();
                    if c.fixes.is_empty() {
                        c = c.advice(format!(
                            "edit the file at {} (a `.material` is source text: no command edits it)",
                            d.location
                        ));
                    }
                    f.cause(c);
                }
            }
            for (output, _) in &status.zero {
                f.cause(Cause::new(
                    codes::RENDER_TRANSPARENT,
                    Likelihood::High,
                    format!("Its `{output}` output is 0, so everything drawn with it is invisible"),
                ));
            }
        }
        let records = facts.diagnostics(&Filter {
            subjects: vec![
                Subject::asset(canonical.clone()),
                Subject::document(canonical.clone()),
            ],
            severity: Some(Severity::Warning),
            ..Filter::default()
        });
        f.check(
            "bus",
            if records.iter().any(|r| r.diagnostic.is_error()) {
                Status::Fail
            } else {
                Status::Ok
            },
            format!("{} reports", records.len()),
        );
        for r in records.iter().rev().take(5) {
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
            c.fixes = super::runnable(&r.diagnostic.fixes);
            f.cause(c);
        }
        if let Some(id) = &asset.id {
            let users = facts.users(id, 6);
            let names: Vec<String> = users
                .iter()
                .take(5)
                .map(|(doc, e, path)| format!("{doc} · {e} · {path}"))
                .collect();
            f.check("used by", Status::Ok, format!("{} fields", users.len()));
            f.related.extend(names);
        }
        f.collect(extra, &Target::Asset(&asset), facts);
        let answer = Report::verdict(&f, "it resolves and nothing is reported about it");
        Ok(Report::new(&q.text, "asset", Some(canonical), answer, f))
    }
}
