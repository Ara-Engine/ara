use ara_schema::Schema;
use serde::Deserialize;

/// Queues derived captures for authored probes without changing their components.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecaptureParams {
    /// Live protocol session that owns the returned job.
    pub session: String,
    /// Scene ID, path or reference; omitted uses the current renderer scene snapshot.
    pub doc: Option<String>,
    /// Probe entity IDs or resolved entity addresses in that scene.
    pub entities: Vec<String>,
    /// Optional explanation retained with this derived-work request.
    pub why: Option<String>,
}

/// Requests cancellation of a probe job owned by this session.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CancelParams {
    /// Session that originally requested the capture.
    pub session: String,
    /// Host job ID returned by render.probe.recapture.
    pub job: String,
}

/// Reads the last owner-validated snapshot without touching the GPU.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct QueryParams {
    /// Live protocol session.
    #[expect(dead_code, reason = "Host validates the session before service dispatch")]
    pub session: String,
    /// Scene ID, path or reference; omitted reads the current renderer scene.
    pub doc: Option<String>,
    /// Optional subset of probe entity IDs or resolved entity addresses.
    pub entities: Option<Vec<String>>,
    /// Return no rows when the snapshot version has not advanced beyond this value.
    pub since: Option<u64>,
}
