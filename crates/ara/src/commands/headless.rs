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
    /// Turn a rendering effect off to see what it contributes (repeatable).
    #[arg(long, value_parser = ["ambient-occlusion", "shadows", "temporal-jitter"])]
    without: Vec<String>,
    /// Render at display resolution instead of the preset's upscaled scale.
    #[arg(long)]
    full_resolution: bool,
    /// Reflection debug view: `reflections` (the reflected light alone) or `sources`
    /// (where each pixel's reflection comes from: on-screen trace green, capture
    /// trace blue, box-projected captures cyan, probes orange, sky magenta).
    #[arg(long, value_parser = ["reflections", "sources"])]
    reflections: Option<String>,
    /// Render the path-traced ground truth (unbiased, undenoised) with this many
    /// samples per pixel, to judge the real-time lighting against. Needs a GPU with
    /// hardware ray tracing; ignores the project's ray-tracing setting.
    #[arg(long, num_args = 0..=1, default_missing_value = "1024")]
    reference: Option<u32>,
    /// Scattering events per reference path (with `--reference`).
    #[arg(long, default_value_t = 8, requires = "reference")]
    bounces: u32,
    /// Lock exposure at this EV100 (manual, no compensation) over every post-process
    /// volume, so captures from different renderers compare at the same brightness.
    #[arg(long, allow_hyphen_values = true)]
    ev100: Option<f32>,
    /// Measure flicker: settle this many live frames, then report how much they change.
    #[arg(long)]
    stability: Option<u32>,
    /// Record a clip of this many live frames (60 per second) into `<name>.video/`.
    #[arg(long)]
    video: Option<u32>,
    /// Camera travel over the clip in metres, `x,y,z`.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    video_move: Option<Vec<f32>>,
    /// Camera turn over the clip in degrees about the vertical axis.
    #[arg(long, allow_hyphen_values = true)]
    video_yaw: Option<f32>,
}
impl CaptureArgs {
    pub fn call(self) -> Call {
        Call {
            method: "capture",
            params: params(json!({"scene":self.scene,"camera":self.camera,
            "path":self.path,"width":self.width,"height":self.height,"preset":self.preset,
            "ticks":self.ticks,"seed":self.seed,"marks":self.marks,"stability":self.stability,
            "video":self.video,"videoMove":self.video_move,"videoYaw":self.video_yaw,
            "renderDebug":render_debug(&self.without, self.full_resolution, self.reflections.as_deref(),
                self.reference.map(|samples| (samples, self.bounces)), self.ev100)})),
            summary: protocol::pretty,
        }
    }
}

/// `capture`'s `renderDebug` for `--without`, `--full-resolution`,
/// `--reflections`, `--reference` and `--ev100`; omitted when every effect stays at
/// its default.
fn render_debug(
    without: &[String],
    full_resolution: bool,
    reflections: Option<&str>,
    reference: Option<(u32, u32)>,
    ev100: Option<f32>,
) -> serde_json::Value {
    if without.is_empty()
        && !full_resolution
        && reflections.is_none()
        && reference.is_none()
        && ev100.is_none()
    {
        return serde_json::Value::Null;
    }
    let on = |name: &str| !without.iter().any(|w| w == name);
    json!({
        "ambientOcclusion": on("ambient-occlusion"),
        "shadows": on("shadows"),
        "temporalJitter": on("temporal-jitter"),
        "fullResolution": full_resolution,
        "reflections": reflections.unwrap_or("lit"),
        "reference": reference.map(|(samples, bounces)| json!({"samples": samples, "bounces": bounces})),
        "exposure": ev100,
    })
}

#[cfg(test)]
mod tests {
    use super::render_debug;
    use serde_json::json;

    #[test]
    fn capture_debug_flags_map_to_render_debug() {
        assert!(render_debug(&[], false, None, None, None).is_null());
        let debug = render_debug(&["shadows".into()], false, None, Some((2048, 6)), Some(-2.));
        assert_eq!(debug["shadows"], false);
        assert_eq!(debug["ambientOcclusion"], true);
        assert_eq!(debug["reference"], json!({"samples": 2048, "bounces": 6}));
        assert_eq!(debug["exposure"], -2.);
        let lock = render_debug(&[], false, None, None, Some(7.));
        assert!(lock["reference"].is_null());
        assert_eq!(lock["exposure"], 7.);
    }
}
