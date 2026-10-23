//! "Why is the frame slow?" — reads the profiler's summary (percentiles, top
//! self-time scopes and GPU passes, hitches with contributors) and over-budget
//! reports on the bus.

use ara_diag::store::Filter;
use ara_diag::{Diagnostic, Fix, FixAction, codes};
use serde_json::{Value, json};

use super::line;
use crate::{Cause, Collector, Explainer, Facts, Findings, Likelihood, Question, Report, Status, Target};

const WORDS: &[&str] = &[
    "frame", "slow", "fps", "hitch", "stutter", "perf", "lag", "jank", "spike", "budget",
];

fn ms(ns: u64) -> String {
    format!("{:.2} ms", ns as f64 / 1e6)
}

/// Explains frame time.
pub struct Frame;

impl Explainer for Frame {
    fn name(&self) -> &'static str {
        "frame"
    }

    fn about(&self) -> &'static str {
        "Why frames are slow: frame-time percentiles against the budget, the top scopes and passes, hitches and their contributors."
    }

    fn examples(&self) -> &'static [&'static str] {
        &["is the frame slow", "frame hitches"]
    }

    fn matches(&self, q: &Question) -> u32 {
        if q.about(WORDS) && q.entities.is_empty() { 85 } else { 0 }
    }

    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        extra: &[&dyn Collector],
    ) -> Result<Report, Vec<Diagnostic>> {
        let mut f = Findings::default();
        let summary = facts.profile().filter(|s| s["frames"].as_u64().unwrap_or(0) > 0);
        let over = facts.diagnostics(&Filter {
            codes: vec![codes::FRAME_OVER_BUDGET],
            ..Filter::default()
        });
        let Some(s) = summary else {
            f.check("profile", Status::Unknown, "nothing captured");
            for r in over.iter().rev().take(3) {
                f.cause(Cause::new(codes::FRAME_OVER_BUDGET, Likelihood::Medium, r.diagnostic.message.clone()).evidence(line(r)));
            }
            f.cause(
                Cause::new(codes::FRAME_OVER_BUDGET, Likelihood::Low, "No frames were profiled yet, so there is nothing to measure".to_owned())
                    .fix(Fix {
                        title: "Start profiling, run the slow part, then ask again".into(),
                        action: FixAction::Call { method: "profile.start".into(), params: json!({}) },
                    }),
            );
            let answer = "no profile capture yet; start one with `profile.start`";
            return Ok(Report::new(&q.text, "frame", None, answer, f));
        };
        let budget = s["budget_ns"].as_u64().unwrap_or(16_666_667);
        let p50 = s["cpu_frame"]["p50"].as_u64().unwrap_or(0);
        let p95 = s["cpu_frame"]["p95"].as_u64().unwrap_or(0);
        let p99 = s["cpu_frame"]["p99"].as_u64().unwrap_or(0);
        let frames = s["frames"].as_u64().unwrap_or(0);
        let slow = p95 > budget;
        f.check(
            "frame time",
            if slow { Status::Fail } else { Status::Ok },
            format!("p50 {} · p95 {} · p99 {} vs budget {} over {frames} frames", ms(p50), ms(p95), ms(p99), ms(budget)),
        );
        let metrics: Vec<&Value> = s["metrics"].as_array().map(|m| m.iter().collect()).unwrap_or_default();
        let mut top: Vec<(&str, u64)> = metrics
            .iter()
            .filter_map(|m| Some((m["key"].as_str()?, m["time"]["p95"].as_u64()?)))
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        if slow {
            if let Some((key, t)) = top.first() {
                let mut c = Cause::new(
                    codes::FRAME_OVER_BUDGET,
                    Likelihood::High,
                    format!("`{key}` takes {} at p95, the most of any scope ({}% of the budget)", ms(*t), t * 100 / budget.max(1)),
                );
                for (k, v) in top.iter().skip(1).take(4) {
                    c = c.evidence(format!("next: {k} {}", ms(*v)));
                }
                f.cause(c);
            }
        }
        let hitches: Vec<&Value> = s["hitches"].as_array().map(|h| h.iter().collect()).unwrap_or_default();
        let count = s["hitch_count"].as_u64().unwrap_or(hitches.len() as u64);
        f.check("hitches", if count > 0 { Status::Fail } else { Status::Ok }, format!("{count}"));
        for h in hitches.iter().take(3) {
            let contributors: Vec<String> = h["contributors"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .take(3)
                        .filter_map(|p| Some(format!("{} {}", p.get(0)?.as_str()?, ms(p.get(1)?.as_u64()?))))
                        .collect()
                })
                .unwrap_or_default();
            let mut c = Cause::new(
                codes::FRAME_OVER_BUDGET,
                if slow { Likelihood::Medium } else { Likelihood::High },
                format!(
                    "Frame {} hitched: {} against {}",
                    h["frame"],
                    ms(h["duration_ns"].as_u64().unwrap_or(0)),
                    ms(h["budget_ns"].as_u64().unwrap_or(budget))
                ),
            )
            .evidence(format!("contributors: {}", contributors.join(", ")));
            for hint in h["hints"].as_array().into_iter().flatten().filter_map(Value::as_str).take(2) {
                c = c.advice(hint);
            }
            f.cause(c);
        }
        if let Some(bg) = s["background_cpu"].as_f64().filter(|b| *b > 0.5) {
            f.cause(Cause::new(
                codes::FRAME_OVER_BUDGET,
                Likelihood::Medium,
                format!("Other processes used {:.0}% of the CPU during the capture", bg * 100.0),
            ));
        }
        for r in over.iter().rev().take(2) {
            f.related.push(line(r));
        }
        f.collect(extra, &Target::Frame, facts);
        let answer = if slow || count > 0 {
            Report::verdict(&f, "frames are slow")
        } else {
            format!("frames are within budget (p95 {} of {})", ms(p95), ms(budget))
        };
        Ok(Report::new(&q.text, "frame", None, answer, f))
    }
}
