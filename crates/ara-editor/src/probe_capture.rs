//! Reflection probe capture has no renderer to run on yet. The protocol methods stay
//! (`render.probe.recapture`, `render.probe.cancel`, `render.probes.query`) so agents and tools
//! that call them get a clear answer, not an unknown-method error: a recapture is rejected,
//! a cancel finds no job, and a query says no snapshot exists. When the Scene's renderer can
//! capture probes, these call it (`studio::unavailable::probe_capture_available`).
mod params;

use ara_diag::Diagnostic;
use ara_host::{Cx, Method, Service};
use ara_schema::SchemaDb;
use params::{RenderProbeCancelParams, RenderProbeRecaptureParams, RenderProbesQueryParams};
use serde_json::Value;

const NOT_AVAILABLE: &str =
    "Reflection probe capture is not available yet: the Scene's renderer cannot capture probes";

/// The probe methods' answer while no renderer captures probes.
#[derive(Default)]
pub(crate) struct ProbeService;

impl Service for ProbeService {
    fn name(&self) -> &'static str {
        "editor-probe-capture"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<RenderProbeRecaptureParams>(
                db,
                "render.probe.recapture",
                "Queue derived reflection-probe captures (not available yet).",
            ),
            Method::new::<RenderProbeCancelParams>(
                db,
                "render.probe.cancel",
                "Cancel this session's reflection-probe capture job (there are none yet).",
            ),
            Method::new::<RenderProbesQueryParams>(
                db,
                "render.probes.query",
                "Read the Scene's probe capabilities and capture receipts (not available yet).",
            )
            .read_only(),
        ]
    }
    fn call(
        &mut self,
        _: &mut Cx<'_>,
        method: &str,
        _: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        match method {
            "render.probe.recapture" | "render.probes.query" => {
                Err(vec![Diagnostic::error(crate::codes::RENDER, NOT_AVAILABLE)])
            }
            "render.probe.cancel" => Err(vec![crate::invalid(
                "No reflection probe capture is running: capture is not available yet",
            )]),
            _ => Err(vec![crate::invalid("Unknown probe capture method")]),
        }
    }
}
