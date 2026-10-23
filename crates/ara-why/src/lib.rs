//! `ara why`: answers "why is X wrong?" with a short ranked list of likely causes,
//! the evidence for each, and runnable fixes.
//!
//! - A [`Question`] is parsed from free text: subjects (`name:Crate`, `e_8b21`,
//!   `@meshes/crate.glb`, `module:Gameplay`), codes (`ARA1302`) and topic words.
//! - [`Explainer`]s are registered per subsystem. Each scores how well it fits a
//!   question ([`Explainer::matches`]); the best one investigates through
//!   [`Facts`] (the host's documents, project files, diagnostics bus and profiler)
//!   and returns a [`Report`].
//! - [`Collector`]s add evidence to an existing explainer without replacing it:
//!   the world streamer adds residency checks to `visibility`, the renderer adds
//!   pass inclusion, and so on.
//!
//! The built-in explainers ([`Why::builtin`]) cover visibility, C# reloads, frame
//! time, diagnostic codes, assets and a diagnostics-about-a-subject fallback. They
//! live here until their subsystems take them over; each is self-contained in
//! `explain/`.

mod explain;
mod facts;
pub mod math;
mod question;
mod report;

pub use explain::{asset, code, frame, reload, subject, visibility};
pub use facts::{Asset, Entity, Facts, ancestors};
pub use question::Question;
pub use report::{Cause, Check, Findings, Likelihood, Report, Status};

use ara_diag::{Code, Diagnostic, FixAction, codes};

/// What an explainer investigates; handed to [`Collector`]s.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    /// A resolved entity.
    Entity(&'a Entity),
    /// A resolved (or unresolved) asset.
    Asset(&'a Asset),
    /// A diagnostic code.
    Code(Code),
    /// C# reloads, optionally of one module.
    Module(Option<&'a str>),
    /// Frame time.
    Frame,
}

/// Explains one kind of question.
pub trait Explainer: Send + Sync {
    /// Short name (`visibility`); collectors name it to join it.
    fn name(&self) -> &'static str;
    /// One line for `ara why --list`.
    fn about(&self) -> &'static str;
    /// Example questions.
    fn examples(&self) -> &'static [&'static str];
    /// How well it fits `q`: 0 = not at all; the highest score answers.
    fn matches(&self, q: &Question) -> u32;
    /// Investigates `q`. `extra` are the collectors registered for this explainer;
    /// run them on the target with [`Findings::collect`].
    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        extra: &[&dyn Collector],
    ) -> Result<Report, Vec<Diagnostic>>;
}

/// Adds evidence to an explainer's investigation.
pub trait Collector: Send + Sync {
    /// The explainer it joins (`visibility`).
    fn explainer(&self) -> &'static str;
    /// Short name, shown in the report's checks (`streaming`).
    fn name(&self) -> &'static str;
    /// Adds checks and causes about `target`.
    fn collect(&self, target: &Target<'_>, facts: &dyn Facts, out: &mut Findings);
}

/// The explainer registry.
#[derive(Default)]
pub struct Why {
    explainers: Vec<Box<dyn Explainer>>,
    collectors: Vec<Box<dyn Collector>>,
}

impl Why {
    /// An empty registry.
    pub fn new() -> Why {
        Why::default()
    }

    /// The built-in explainers: visibility, reload, frame, code, asset and the
    /// subject fallback.
    pub fn builtin() -> Why {
        let mut why = Why::new();
        for e in explain::builtin() {
            why.register(e);
        }
        why
    }

    /// Adds an explainer. Earlier ones win score ties.
    pub fn register(&mut self, explainer: Box<dyn Explainer>) {
        self.explainers.push(explainer);
    }

    /// Adds a collector to the explainer it names.
    pub fn add(&mut self, collector: Box<dyn Collector>) {
        self.collectors.push(collector);
    }

    /// `(name, about, examples)` of every explainer.
    pub fn list(&self) -> Vec<(&'static str, &'static str, &'static [&'static str])> {
        self.explainers
            .iter()
            .map(|e| (e.name(), e.about(), e.examples()))
            .collect()
    }

    /// Answers `question` with the best-fitting explainer.
    pub fn explain(&self, question: &str, facts: &dyn Facts) -> Result<Report, Vec<Diagnostic>> {
        let q = Question::parse(question);
        let mut best: Option<(u32, &Box<dyn Explainer>)> = None;
        for e in &self.explainers {
            let score = e.matches(&q);
            if score > 0 && best.is_none_or(|(b, _)| score > b) {
                best = Some((score, e));
            }
        }
        let Some((_, explainer)) = best else {
            return Err(vec![self.unanswered(question)]);
        };
        let extra: Vec<&dyn Collector> = self
            .collectors
            .iter()
            .filter(|c| c.explainer() == explainer.name())
            .map(Box::as_ref)
            .collect();
        explainer.explain(&q, facts, &extra)
    }

    fn unanswered(&self, question: &str) -> Diagnostic {
        let examples: Vec<&str> = self
            .explainers
            .iter()
            .filter_map(|e| e.examples().first().copied())
            .collect();
        let mut d = Diagnostic::error(
            codes::WHY_UNANSWERED,
            format!(
                "no explainer understands `{question}`; name a subject, a code or a topic, like: {}",
                examples.join(" · ")
            ),
        );
        if let Some(example) = examples.first() {
            d = d.with_fix(
                format!("Ask `{example}`"),
                FixAction::Cli {
                    args: std::iter::once("why".to_owned())
                        .chain(example.split_whitespace().map(str::to_owned))
                        .collect(),
                },
            );
        }
        d
    }
}
