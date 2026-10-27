//! CLI bindings for the same headless methods exposed through MCP.
use crate::{
    commands::protocol::{self, Call},
    params,
};
use serde_json::json;

#[derive(clap::Args)]
pub struct RunArgs {
    /// Scene path or ID (default: project main scene).
    #[arg(long)]
    scene: Option<String>,
    /// Exact fixed ticks (default 60).
    #[arg(long, conflicts_with = "duration")]
    ticks: Option<u32>,
    /// Simulated seconds, rounded up to a fixed tick.
    #[arg(long)]
    duration: Option<f64>,
    /// Deterministic seed.
    #[arg(long)]
    seed: Option<u64>,
    /// Fixed ticks per second.
    #[arg(long)]
    tick_hz: Option<u32>,
}
impl RunArgs {
    pub fn call(self) -> Call {
        Call {
            method: "run",
            params: params(json!({"scene":self.scene,"ticks":self.ticks,
            "duration":self.duration,"seed":self.seed,"tickHz":self.tick_hz})),
            summary: protocol::pretty,
        }
    }
}

#[derive(clap::Args)]
pub struct CaptureArgs {
    /// Scene path or ID (default: project main scene).
    #[arg(long)]
    scene: Option<String>,
    /// Camera entity ID or exact name.
    #[arg(long)]
    camera: Option<String>,
    /// PNG output path (default: .ara/captures/).
    #[arg(long, alias = "out")]
    path: Option<String>,
    /// Image width in pixels.
    #[arg(long)]
    width: Option<u32>,
    /// Image height in pixels.
    #[arg(long)]
    height: Option<u32>,
    /// Rendering preset: auto, low, medium, high or epic.
    #[arg(long, value_parser = ["auto", "low", "medium", "high", "epic"])]
    preset: Option<String>,
    /// Fixed ticks before capture.
    #[arg(long)]
    ticks: Option<u32>,
    /// Deterministic seed.
    #[arg(long)]
    seed: Option<u64>,
    /// Maximum numbered entity marks (zero disables them).
    #[arg(long)]
    marks: Option<u32>,
}
impl CaptureArgs {
    pub fn call(self) -> Call {
        Call {
            method: "capture",
            params: params(json!({"scene":self.scene,"camera":self.camera,
            "path":self.path,"width":self.width,"height":self.height,"preset":self.preset,
            "ticks":self.ticks,"seed":self.seed,"marks":self.marks})),
            summary: protocol::pretty,
        }
    }
}
