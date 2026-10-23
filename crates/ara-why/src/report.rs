//! What an explanation says: ranked causes with evidence and fixes, the checks
//! that ruled things out, and related diagnostics. JSON for agents, compact text
//! for both (agents pay per token).

use ara_diag::{Code, Fix, FixAction, Subject};
use serde::Serialize;
use serde_json::Value;

use crate::{Collector, Facts, Target};

/// The outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Checked and fine.
    Ok,
    /// Checked and wrong: there is a cause for it.
    Fail,
    /// Does not apply here (no material set, not a streamed world).
    Skip,
    /// Cannot be checked from here (needs the running renderer).
    Unknown,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Fail => "FAIL",
            Status::Skip => "n/a",
            Status::Unknown => "?",
        }
    }
}

/// One thing that was checked.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    /// What (`transform`, `layer`).
    pub name: String,
    /// The outcome.
    pub status: Status,
    /// A few words of detail (`Camera e_cam01`, `12 m`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// How likely a cause is the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Likelihood {
    /// Certain from the data: on its own it explains the symptom.
    High,
    /// Probably involved.
    Medium,
    /// Possible; worth a look if nothing above explains it.
    Low,
}

/// One likely cause.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cause {
    /// The code that names this cause (its docs: `ara why <code>`).
    pub code: Code,
    /// How likely.
    pub likelihood: Likelihood,
    /// What is wrong, in one sentence.
    pub title: String,
    /// The facts behind it (`e_stack01 Transform.scale = [1, 0, 1]`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// What it involves.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<Subject>,
    /// Runnable fixes, best first (`ara_apply` commands, protocol calls, CLI lines).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Fix>,
    /// Steps that are not a single command.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub advice: Vec<String>,
}

impl Cause {
    /// A cause with a title.
    pub fn new(code: Code, likelihood: Likelihood, title: impl Into<String>) -> Cause {
        Cause {
            code,
            likelihood,
            title: title.into(),
            evidence: Vec::new(),
            subjects: Vec::new(),
            fixes: Vec::new(),
            advice: Vec::new(),
        }
    }

    /// Adds evidence.
    pub fn evidence(mut self, line: impl Into<String>) -> Cause {
        self.evidence.push(line.into());
        self
    }

    /// Adds a subject.
    pub fn subject(mut self, s: Subject) -> Cause {
        if !self.subjects.contains(&s) {
            self.subjects.push(s);
        }
        self
    }

    /// Adds a runnable fix.
    pub fn fix(mut self, fix: Fix) -> Cause {
        self.fixes.push(fix);
        self
    }

    /// Adds a step.
    pub fn advice(mut self, step: impl Into<String>) -> Cause {
        self.advice.push(step.into());
        self
    }
}

/// Checks and causes gathered during an investigation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Findings {
    /// What was checked, in order.
    pub checks: Vec<Check>,
    /// Likely causes, in discovery order (ranked when the report is built).
    pub causes: Vec<Cause>,
    /// Related diagnostics, one line each.
    pub related: Vec<String>,
}

impl Findings {
    /// Records a check.
    pub fn check(&mut self, name: &str, status: Status, note: impl Into<String>) {
        self.checks.push(Check {
            name: name.to_owned(),
            status,
            note: note.into(),
        });
    }

    /// Records a cause.
    pub fn cause(&mut self, cause: Cause) {
        self.causes.push(cause);
    }

    /// Runs registered collectors on `target`.
    pub fn collect(&mut self, extra: &[&dyn Collector], target: &Target<'_>, facts: &dyn Facts) {
        for c in extra {
            c.collect(target, facts, self);
        }
    }

    /// True if a check of this name failed.
    pub fn failed(&self, name: &str) -> bool {
        self.checks
            .iter()
            .any(|c| c.name == name && c.status == Status::Fail)
    }
}

/// An answer to one question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// The question as asked.
    pub question: String,
    /// Which explainer answered.
    pub explainer: &'static str,
    /// What it is about, resolved (`Crate (e_crate001) in levels/outpost.scene`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The verdict in one line.
    pub answer: String,
    /// Likely causes, most likely first.
    pub causes: Vec<Cause>,
    /// What was checked.
    pub checks: Vec<Check>,
    /// Related diagnostics, one line each.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<String>,
    /// Reference documentation (a code's page).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

impl Report {
    /// Builds a report from findings, ranking causes by likelihood (stable).
    pub fn new(
        question: &str,
        explainer: &'static str,
        subject: Option<String>,
        answer: impl Into<String>,
        mut findings: Findings,
    ) -> Report {
        findings.causes.sort_by_key(|c| c.likelihood);
        Report {
            question: question.to_owned(),
            explainer,
            subject,
            answer: answer.into(),
            causes: findings.causes,
            checks: findings.checks,
            related: findings.related,
            doc: None,
        }
    }

    /// The default verdict: how many likely causes, or that nothing was found.
    pub fn verdict(findings: &Findings, nothing: &str) -> String {
        let strong = findings
            .causes
            .iter()
            .filter(|c| c.likelihood == Likelihood::High)
            .count();
        match (findings.causes.len(), strong) {
            (0, _) => nothing.to_owned(),
            (1, _) => "1 likely cause".to_owned(),
            (n, 0) => format!("{n} possible causes"),
            (n, s) => format!("{n} causes, {s} certain"),
        }
    }

    /// Compact text: the verdict, numbered causes with evidence and fixes, then the
    /// checks on one line.
    pub fn text(&self) -> String {
        let mut s = match &self.subject {
            Some(subject) => format!("{subject}: {}\n", self.answer),
            None => format!("{}\n", self.answer),
        };
        for (i, c) in self.causes.iter().enumerate() {
            let level = match c.likelihood {
                Likelihood::High => "high",
                Likelihood::Medium => "medium",
                Likelihood::Low => "low",
            };
            s.push_str(&format!("{}. [{level}] {} ({})\n", i + 1, c.title, c.code));
            for e in &c.evidence {
                s.push_str(&format!("   evidence: {e}\n"));
            }
            for f in &c.fixes {
                s.push_str(&format!("   fix: {}\n", fix_line(f)));
            }
            for a in &c.advice {
                s.push_str(&format!("   then: {a}\n"));
            }
        }
        if !self.checks.is_empty() {
            let checks: Vec<String> = self
                .checks
                .iter()
                .map(|c| {
                    if c.note.is_empty() {
                        format!("{} {}", c.name, c.status.word())
                    } else {
                        format!("{} {} ({})", c.name, c.status.word(), c.note)
                    }
                })
                .collect();
            s.push_str(&format!("checked: {}\n", checks.join(", ")));
        }
        if !self.related.is_empty() {
            s.push_str("related:\n");
            for r in &self.related {
                s.push_str(&format!("- {r}\n"));
            }
        }
        if let Some(doc) = &self.doc {
            s.push('\n');
            s.push_str(doc.trim_end());
            s.push('\n');
        }
        s
    }
}

/// A fix as one line: `property.set {"entity":"e_1","path":"Transform.scale",...}`.
pub fn fix_line(f: &Fix) -> String {
    let compact = |v: &Value| serde_json::to_string(v).unwrap_or_default();
    match &f.action {
        FixAction::Command { command, params } => {
            format!("{} — ara_apply {command} {}", f.title, compact(params))
        }
        FixAction::Call { method, params } => {
            format!("{} — call {method} {}", f.title, compact(params))
        }
        FixAction::Cli { args } => format!("{} — `ara {}`", f.title, args.join(" ")),
        FixAction::Set {
            record,
            path,
            value,
        } => format!(
            "{} — set {}{path} = {}",
            f.title,
            record
                .as_deref()
                .map(|r| format!("{r} "))
                .unwrap_or_default(),
            compact(value)
        ),
        FixAction::Remove { record, path } => format!(
            "{} — remove {}{path}",
            f.title,
            record
                .as_deref()
                .map(|r| format!("{r} "))
                .unwrap_or_default()
        ),
        FixAction::Rename { record, path, to } => format!(
            "{} — rename {}{path} to {to}",
            f.title,
            record
                .as_deref()
                .map(|r| format!("{r} "))
                .unwrap_or_default()
        ),
    }
}
