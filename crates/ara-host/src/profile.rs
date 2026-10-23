//! Typed Engine Protocol service for profiling. No authoring documents are mutated.
use crate::{Cx, Method, Service, parse};
use ara_diag::{Diagnostic, codes};
use ara_profile::{Summary, SummaryBudget};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

/// Parameters shared by profiling control and summary queries.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileParams {
    /// Calling Engine Protocol session.
    session: String,
    /// Maximum metric rows (default 20, hard maximum 1024).
    top_n: Option<u32>,
    /// Maximum hitch records (default 5, hard maximum 1024).
    limit: Option<u32>,
    /// Maximum compact JSON response bytes (default 16384).
    max_bytes: Option<u32>,
}
/// Compare summaries supplied as data, avoiding arbitrary filesystem access.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompareParams {
    /// Calling Engine Protocol session.
    session: String,
    /// Saved baseline summary as a JSON string.
    baseline: String,
    /// Current summary as a JSON string; omitted uses the live rolling capture.
    current: Option<String>,
    /// Allowed fractional p95 increase (default 0.05).
    relative: Option<f64>,
    /// Allowed absolute increase in nanoseconds (default 1000).
    absolute_ns: Option<u64>,
}
pub(crate) struct ProfileService;
fn error(e: impl std::fmt::Display) -> Vec<Diagnostic> {
    vec![Diagnostic::error(codes::INVALID_PARAMS, e.to_string())]
}
fn summary(text: &str) -> Result<Summary, Vec<Diagnostic>> {
    serde_json::from_value(ara_json::parse(text).map_err(error)?).map_err(error)
}
impl Service for ProfileService {
    fn name(&self) -> &'static str {
        "profile"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<ProfileParams>(db, "profile.start", "Start bounded frame profiling.")
                .action("ara_profile", "start"),
            Method::new::<ProfileParams>(
                db,
                "profile.stop",
                "Stop profiling and collect completed spans.",
            )
            .action("ara_profile", "stop"),
            Method::new::<ProfileParams>(
                db,
                "profile.summary",
                "Budgeted CPU/GPU percentiles and top self-time scopes.",
            )
            .read_only()
            .action("ara_profile", "summary"),
            Method::new::<ProfileParams>(
                db,
                "profile.hitches",
                "Over-budget frames and measured contributors.",
            )
            .read_only()
            .action("ara_profile", "hitches"),
            Method::new::<CompareParams>(
                db,
                "profile.compare",
                "Compare p95 against a saved baseline.",
            )
            .read_only()
            .action("ara_profile", "compare"),
        ]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        if method == "profile.compare" {
            let p: CompareParams = parse(params)?;
            let _ = p.session;
            let baseline = summary(&p.baseline)?;
            let current = match p.current {
                Some(text) => summary(&text)?,
                None => cx
                    .st
                    .profiler
                    .profile()
                    .summary(SummaryBudget {
                        top_n: 1024,
                        hitches: 0,
                        max_bytes: 1_048_576,
                    })
                    .map_err(error)?,
            };
            return serde_json::to_value(
                ara_profile::compare(
                    &baseline,
                    &current,
                    p.relative.unwrap_or(0.05),
                    p.absolute_ns.unwrap_or(1000),
                )
                .map_err(error)?,
            )
            .map_err(error);
        }
        let p: ProfileParams = parse(params)?;
        let _ = p.session;
        let budget = SummaryBudget {
            top_n: p.top_n.unwrap_or(20) as usize,
            hitches: p.limit.unwrap_or(5) as usize,
            max_bytes: p.max_bytes.unwrap_or(16_384) as usize,
        };
        match method {
            "profile.start" => {
                if !cx.st.profile_attached && !cx.st.profiler.current_thread_attached() {
                    cx.st
                        .profiler
                        .register_thread("host")
                        .attach()
                        .map_err(error)?;
                    cx.st.profile_attached = true;
                }
                cx.st.profiler.start();
                Ok(json!({"recording": true}))
            }
            "profile.stop" => {
                cx.st.profiler.stop();
                cx.st.profiler.collect();
                Ok(json!({"recording": false}))
            }
            "profile.summary" | "profile.hitches" => {
                cx.st.profiler.collect();
                let mut result = cx.st.profiler.profile().summary(budget).map_err(error)?;
                if method == "profile.hitches" {
                    result.metrics.clear();
                }
                serde_json::to_value(result).map_err(error)
            }
            _ => Err(error("unknown profile method")),
        }
    }
}
