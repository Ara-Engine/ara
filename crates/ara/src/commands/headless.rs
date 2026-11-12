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
    #[arg(long, value_parser = ["ambient-occlusion", "shadows", "soft-shadows", "temporal-jitter",
        "screen-reflections", "reflection-captures", "global-illumination", "screen-gi", "volumetric-fog"])]
    without: Vec<String>,
    /// Render at display resolution instead of the preset's upscaled scale.
    #[arg(long)]
    full_resolution: bool,
    /// Reflection debug view: `reflections` (the reflected light alone) or `sources`
    /// (where each pixel's reflection comes from: on-screen trace green, capture
    /// trace blue, screen probes yellow, box-projected captures cyan, probes orange,
    /// sky magenta).
    #[arg(long, value_parser = ["reflections", "sources"])]
    reflections: Option<String>,
    /// Lock exposure at this EV100 (manual, no compensation) over every post-process
    /// volume, so captures from different renderers compare at the same brightness.
    /// Without it a scene with no post-process volume is metered automatically like one
    /// with a default volume (not the legacy daylight exposure), and the capture JSON
    /// reports the exposure applied.
    #[arg(long, allow_hyphen_values = true)]
    ev100: Option<f32>,
    /// Debug view to capture instead of the lit image: albedo, normals, depth,
    /// roughness-metal, entity-ids, shadow-cascades, wireframe, lit-wireframe,
    /// gi-diffuse, overdraw, vertex-color (and -red/-green/-blue/-alpha),
    /// decal-coverage, light-complexity or shadow-penumbra.
    #[arg(long, value_parser = ["lit", "albedo", "normals", "depth", "roughness-metal", "entity-ids", "shadow-cascades", "wireframe", "lit-wireframe", "gi-diffuse", "overdraw", "vertex-color", "vertex-color-red", "vertex-color-green", "vertex-color-blue", "vertex-color-alpha", "decal-coverage", "light-complexity", "shadow-penumbra"])]
    view: Option<String>,
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
    /// Cut the camera once inside the clip: from this clip frame (0-based) on, the camera
    /// jumps by `--video-cut-move` and turns by `--video-cut-yaw` on top of its path.
    #[arg(long, requires = "video")]
    video_cut_at: Option<u32>,
    /// World-space jump of the cut in metres, `x,y,z`.
    #[arg(
        long,
        value_delimiter = ',',
        allow_hyphen_values = true,
        requires = "video_cut_at"
    )]
    video_cut_move: Option<Vec<f32>>,
    /// Turn of the cut in degrees about the vertical axis.
    #[arg(long, allow_hyphen_values = true, requires = "video_cut_at")]
    video_cut_yaw: Option<f32>,
}
impl CaptureArgs {
    pub fn call(self) -> Call {
        Call {
            method: "capture",
            params: params(json!({"scene":self.scene,"camera":self.camera,
            "path":self.path,"width":self.width,"height":self.height,"preset":self.preset,
            "ticks":self.ticks,"seed":self.seed,"marks":self.marks,"stability":self.stability,
            "video":self.video,"videoMove":self.video_move,"videoYaw":self.video_yaw,
            "videoCutAt":self.video_cut_at,"videoCutMove":self.video_cut_move,"videoCutYaw":self.video_cut_yaw,
            "renderDebug":render_debug(&self.without, self.full_resolution, self.reflections.as_deref(),
                self.ev100, self.view.as_deref())})),
            summary: protocol::pretty,
        }
    }
}

/// A kebab-case flag value in camelCase (`gi-diffuse` -> `giDiffuse`).
fn camel(value: &str) -> String {
    let mut parts = value.split('-');
    let mut out = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// `capture`'s `renderDebug` for `--without`, `--full-resolution`,
/// `--reflections`, `--ev100` and `--view`; omitted when every effect stays at
/// its default. The path-traced reference is gone; the image is the player's.
fn render_debug(
    without: &[String],
    full_resolution: bool,
    reflections: Option<&str>,
    ev100: Option<f32>,
    view: Option<&str>,
) -> serde_json::Value {
    if without.is_empty()
        && !full_resolution
        && reflections.is_none()
        && ev100.is_none()
        && view.is_none()
    {
        return serde_json::Value::Null;
    }
    let on = |name: &str| !without.iter().any(|w| w == name);
    json!({
        "ambientOcclusion": on("ambient-occlusion"),
        "shadows": on("shadows"),
        "softShadows": on("soft-shadows"),
        "volumetricFog": on("volumetric-fog"),
        "temporalJitter": on("temporal-jitter"),
        "screenReflections": on("screen-reflections"),
        "reflectionCaptures": on("reflection-captures"),
        "globalIllumination": on("global-illumination"),
        "screenGi": on("screen-gi"),
        "fullResolution": full_resolution,
        "reflections": reflections.unwrap_or("lit"),
        "exposure": ev100,
        // kebab-case flag values name the camelCase BufferView variants.
        "view": view.map_or_else(|| "lit".to_owned(), camel),
    })
}

#[cfg(test)]
mod tests {
    use super::render_debug;

    #[test]
    fn capture_debug_flags_map_to_render_debug() {
        assert!(render_debug(&[], false, None, None, None).is_null());
        let debug = render_debug(&["shadows".into()], false, None, Some(-2.), None);
        assert_eq!(debug["shadows"], false);
        assert_eq!(debug["ambientOcclusion"], true);
        assert_eq!(debug["volumetricFog"], true);
        assert_eq!(debug["exposure"], -2.);
        let lock = render_debug(&[], false, None, Some(7.), Some("gi-diffuse"));
        assert_eq!(lock["exposure"], 7.);
        assert_eq!(lock["view"], "giDiffuse");
        assert_eq!(debug["view"], "lit");
    }

    /// `ara capture` parsed from `args`, as the protocol call it makes.
    fn capture_call(args: &[&str]) -> Result<serde_json::Value, clap::Error> {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            capture: super::CaptureArgs,
        }
        let cli = <Cli as clap::Parser>::try_parse_from(
            std::iter::once("capture").chain(args.iter().copied()),
        )?;
        Ok(cli.capture.call().params)
    }

    #[test]
    fn capture_cut_flags_reach_the_request() {
        let params = capture_call(&[
            "--video",
            "120",
            "--video-move",
            "4,0,-4",
            "--video-yaw",
            "12",
            "--video-cut-at",
            "60",
            "--video-cut-move",
            "-20,0,15",
            "--video-cut-yaw",
            "140",
        ])
        .expect("parses");
        assert_eq!(params["videoCutAt"], 60);
        assert_eq!(params["videoCutMove"], serde_json::json!([-20., 0., 15.]));
        assert_eq!(params["videoCutYaw"], 140.);
        // No cut flags, no cut params.
        let plain = capture_call(&["--video", "6"]).expect("parses");
        assert!(plain.get("videoCutAt").is_none() && plain.get("videoCutMove").is_none());
    }

    #[test]
    fn a_cut_needs_a_clip_and_its_frame() {
        assert!(capture_call(&["--video-cut-at", "3"]).is_err());
        assert!(capture_call(&["--video", "10", "--video-cut-yaw", "90"]).is_err());
        assert!(
            capture_call(&[
                "--video",
                "10",
                "--video-cut-at",
                "3",
                "--video-cut-yaw",
                "90"
            ])
            .is_ok()
        );
    }
}
