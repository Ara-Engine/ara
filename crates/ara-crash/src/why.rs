//! Crash explainer registered by the host composition root.
use ara_why::{Cause, Collector, Explainer, Facts, Likelihood, Question, Report};

/// Explains the local crash diagnostics already collected by the host.
pub struct CrashExplainer;
impl Explainer for CrashExplainer {
    fn name(&self) -> &'static str {
        "crash"
    }
    fn about(&self) -> &'static str {
        "Explain native, Rust panic and managed crash stacks"
    }
    fn examples(&self) -> &'static [&'static str] {
        &["why did the player crash"]
    }
    fn matches(&self, q: &Question) -> u32 {
        if q.text.to_lowercase().contains("crash") || q.codes.contains(&crate::CRASH) {
            200
        } else {
            0
        }
    }
    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        _extra: &[&dyn Collector],
    ) -> Result<Report, Vec<ara_diag::Diagnostic>> {
        let records = facts.diagnostics(&ara_diag::store::Filter {
            codes: vec![crate::CRASH],
            ..Default::default()
        });
        let causes = records.iter().filter_map(|record| {
            let crash = record.diagnostic.data.as_ref()?.get("crash")?;
            let report = serde_json::from_value::<crate::CrashReport>(crash.clone()).ok()?;
            Some(Cause { code: crate::CRASH, likelihood: Likelihood::High, title: report.message, evidence: vec![report.stack], subjects: vec![], fixes: vec![], advice: vec![format!("Use the symbols archive matching build {}. Start with the exception message and gameplay frames; the stack records the call path, and the root cause still needs investigation.", report.build_id)] })
        }).collect::<Vec<_>>();
        Ok(Report {
            question: q.text.clone(),
            explainer: "crash",
            subject: None,
            answer: if causes.is_empty() {
                "No symbolized crash has been reported. Run ara crash symbolize <dump> with the matching symbols and report its diagnostic to the host.".into()
            } else {
                format!(
                    "{} local crash report(s); inspect the application frames and exception message.",
                    causes.len()
                )
            },
            causes,
            checks: vec![],
            related: vec![],
            doc: None,
        })
    }
}
