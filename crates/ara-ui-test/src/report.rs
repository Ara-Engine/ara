//! Reflected `capture.ui` wire report; schema generation shares the engine's Schema DB.
use ara_schema::{Schema, SchemaDb};
use ara_ui_stories::StoryView;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Versioned evidence for one PNG, independent of protocol or filesystem transport.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureReport {
    /// Report contract version; currently 1.
    pub version: u32,
    /// Complete size, DPI, locale, input, theme and clock configuration.
    pub view: StoryView,
    /// Story/component name, or absent for a plain document capture.
    pub story: Option<String>,
    /// Registered fixture schema type, or absent for a plain document.
    pub fixture_type: Option<String>,
    /// Actual physical PNG width and height.
    pub pixels: Vec<u32>,
    /// All retained nodes, including hidden nodes explicitly marked invisible.
    pub nodes: Vec<NodeReport>,
    /// Visible, enabled tab and directional navigation links.
    pub focus_graph: Vec<FocusLink>,
    /// Actual renderer batches, in painter order, including their break causes.
    pub batches: Vec<BatchReport>,
    /// Actionable overflow, clipping, font, contrast and touch-target diagnostics.
    pub diagnostics: Vec<UiDiagnostic>,
    /// Retained runtime's restyle/layout/shaping/paint work counters.
    pub work: Value,
    /// Display-list command count before renderer batching.
    pub commands: u32,
    /// Positioned glyph count.
    pub glyphs: u32,
    /// Focused stable authored identity, if an input scope exists.
    pub focused: Option<String>,
}

/// A logical-pixel rectangle; DPI scaling applies only to the PNG/render stream.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Rect {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}
impl From<ara_ui::paint::Box2> for Rect {
    fn from(r: ara_ui::paint::Box2) -> Self {
        Self {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    }
}

/// One semantic retained node, its measured geometry and resolved style evidence.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeReport {
    /// Compiled stable authored identity.
    pub id: String,
    /// Source tag.
    pub tag: String,
    /// Parent authored identity, or absent for the root.
    pub parent: Option<String>,
    /// Accessible Testing Library role.
    pub role: String,
    /// Resolved localized accessible name.
    pub label: String,
    /// Whether the node intersects the viewport and its ancestor clips.
    pub visible: bool,
    /// Current display text, including visual-only IME preedit.
    pub text: Option<String>,
    /// Committed native widget value; null for valueless nodes.
    pub value: Value,
    /// Computed box in logical viewport pixels, after animation and RTL.
    pub bounds: Rect,
    /// Intersected ancestor clip bounds.
    pub clip: Rect,
    /// Measured text content bounds, before clipping.
    pub text_bounds: Option<Rect>,
    /// Resolved native style properties.
    pub style: ComputedStyle,
    /// Source declarations in cascade order: selector, property, value, source spans and inline overrides.
    pub provenance: Vec<Value>,
    /// Native shaping coverage, bidi directions and original UTF-8 clusters.
    pub shaping: Option<Value>,
}

/// Computed paint/style values alongside the layout that produced the box.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputedStyle {
    /// Straight-alpha sRGB background channels.
    pub background: Vec<u8>,
    /// Straight-alpha sRGB text channels.
    pub foreground: Vec<u8>,
    /// Straight-alpha sRGB border channels.
    pub border_color: Vec<u8>,
    /// Logical font size.
    pub font_size: f32,
    /// Corner radius in logical pixels.
    pub radius: f32,
    /// Border width in logical pixels.
    pub border_width: f32,
    /// Actual Taffy display mode.
    pub display: String,
    /// Actual Taffy flex direction.
    pub flex_direction: String,
    /// Whether this node clips its contents.
    pub clips: bool,
    /// Resolved top/right/bottom/left padding.
    pub padding: Vec<f32>,
}

/// One directed native focus graph vertex.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FocusLink {
    /// Source control identity.
    pub id: String,
    /// Next tab control; wraps inside the active surface.
    pub tab: Option<String>,
    /// Left neighbor, respecting explicit author overrides.
    pub left: Option<String>,
    /// Right neighbor.
    pub right: Option<String>,
    /// Up neighbor.
    pub up: Option<String>,
    /// Down neighbor.
    pub down: Option<String>,
}

/// The actual CPU adapter's renderer batch boundary, after atlas allocation and DPI scaling.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchReport {
    /// Native shader pipeline name.
    pub pipeline: String,
    /// Atlas/image resource handle.
    pub texture: u32,
    /// Clip-chain arena index.
    pub clip: u32,
    /// First instance offset.
    pub first: u32,
    /// Number of instances in this batch.
    pub count: u32,
    /// Actual break reason: first draw, clip, pipeline or atlas/image change.
    pub cause: String,
}

/// Stable machine-readable diagnosis with an actionable fix and optional numeric evidence.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct UiDiagnostic {
    /// Catalog code, including ARA4005 identity collisions and ARA4100..ARA4106 inspection findings.
    pub code: String,
    /// Source node identity.
    pub node: String,
    /// Human-readable finding, including locale context where relevant.
    pub message: String,
    /// Suggested source/model/font fix; never silently shrinks accessibility text.
    pub suggested_fix: String,
    /// Measured ratio, minimum dimension or glyph count when meaningful.
    pub measured: Option<f64>,
    /// Required minimum where applicable.
    pub threshold: Option<f64>,
    /// True when image/transparency/coverage prevents a defensible contrast ratio.
    pub uncertain: bool,
}

/// Register the capture contract and all reachable types for protocol and inspector generation.
pub fn register(db: &mut SchemaDb) {
    db.register::<CaptureReport>();
}

/// Generate a self-contained JSON Schema 2020-12 report contract from reflected Rust types.
pub fn report_schema() -> Value {
    let mut db = SchemaDb::new();
    register(&mut db);
    db.type_json_schema(&CaptureReport::describe())
}
