use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// An isolated component example, serialized as an `st_` record.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
#[ara(record(prefix = "st"), module = "ui")]
pub struct Story {
    /// Human-readable example name.
    pub name: String,
    /// Component or screen being demonstrated, using `@path#id` for external assets.
    pub component: String,
    /// Original native markup, including optional component templates.
    pub markup: String,
    /// Token-only stylesheet appended to Ara's standard theme rules.
    pub stylesheet: String,
    /// Registered schema type of the fixture model.
    pub fixture_type: String,
    /// Immutable model values used to initialize `bind` destinations.
    pub fixture: BTreeMap<String, Value>,
    /// Explicit locale packs; every `loc:key` must be present.
    pub locales: Vec<Locale>,
    /// Representative captures, with all dimensions explicit rather than an implicit host default.
    pub views: Vec<StoryView>,
    /// Deterministic translation tracks sampled at each view's animation time.
    pub transitions: Vec<Transition>,
    /// Self-contained decoded image fixtures; production hosts can supply their own resources.
    pub images: Vec<StoryImage>,
    /// Reproducible semantic interactions and assertions, executed after the initial capture.
    pub steps: Vec<StoryStep>,
}

/// One fixed native preview configuration; dimensions are logical pixels and DPI is pixels/inch.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct StoryView {
    /// Logical viewport width.
    pub width: u32,
    /// Logical viewport height.
    pub height: u32,
    /// Physical scale; 96 means one physical pixel per logical pixel.
    pub dpi: u32,
    /// Locale pack identifier from the story.
    pub locale: String,
    /// Device used by semantic activation steps and action prompts.
    pub input: InputMode,
    /// Semantic Ara palette.
    pub theme: Theme,
    /// Absolute deterministic UI animation time, in milliseconds.
    pub time_ms: u32,
}

/// Input device used by a story's activation recipe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "camelCase")]
pub enum InputMode {
    /// Primary pointer hover, press and release.
    #[default]
    Mouse,
    /// Focus followed by keyboard activation.
    Keyboard,
    /// Focus followed by controller confirmation.
    Gamepad,
}

/// Ara's D18 semantic theme selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    /// T3 Code's dark palette.
    #[default]
    Dark,
    /// T3 Code's light palette.
    Light,
}

/// An explicit translation pack with optional pseudo expansion and layout mirroring.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Locale {
    /// Locale identifier, for example `en`, `ar` or `qps-ploc`.
    pub id: String,
    /// Mirror layout boxes while preserving glyph direction and logical reading order.
    pub rtl: bool,
    /// Apply accent substitution and 35 percent expansion after resolving messages.
    pub pseudo: bool,
    /// Translation keys and complete localized strings.
    pub messages: BTreeMap<String, String>,
}

/// A deterministic paint translation; never changes gameplay state or the wall clock.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Transition {
    /// Stable compiled node identity.
    pub node: String,
    /// Translation at the start time, in logical pixels.
    pub from: [f32; 2],
    /// Translation at the end time, in logical pixels.
    pub to: [f32; 2],
    /// Absolute start time in milliseconds.
    pub start_ms: u32,
    /// Duration in milliseconds; zero is an immediate change.
    pub duration_ms: u32,
}

/// Decoded RGBA8 image fixture, resolved by the same source handle as runtime images.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct StoryImage {
    /// Image `src` string, normally an `@path#id` reference.
    pub source: String,
    /// Source bitmap width.
    pub width: u32,
    /// Source bitmap height.
    pub height: u32,
    /// Straight-alpha RGBA8 pixels in row order.
    pub rgba: Vec<u8>,
}

/// A visible semantic query; ambiguity is an error rather than an arbitrary first match.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    /// Semantic attribute to match.
    pub by: QueryBy,
    /// Exact role, localized label/text or test ID.
    pub value: String,
    /// Optional accessible name, used to distinguish controls with the same role.
    pub name: Option<String>,
}

/// Supported Testing Library-style selectors.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(rename_all = "camelCase")]
pub enum QueryBy {
    /// Accessible role, using names such as `button`, `textbox`, `checkbox` and `dialog`.
    #[default]
    Role,
    /// Accessible localized name.
    Label,
    /// Exact currently displayed localized text.
    Text,
    /// Explicit `test-id` attribute, falling back to the stable authored ID.
    TestId,
}

/// One declarative action or assertion in a component story.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct StoryStep {
    /// Operation to execute through the production input seam.
    pub kind: StepKind,
    /// Target control; unused by time and named-action assertions.
    pub query: Query,
    /// Expected value, inserted text, action name or elapsed milliseconds, according to kind.
    pub value: Value,
}

/// Operations supported by the data-only story test runner.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    /// Require exactly one visible semantic match.
    #[default]
    Visible,
    /// Focus a target through the native scope manager.
    Focus,
    /// Require that the target owns native focus.
    Focused,
    /// Activate via the view's pointer, keyboard or controller mode.
    Activate,
    /// Insert text through the native text editing seam.
    Text,
    /// Assert a committed native control value.
    Value,
    /// Assert current visible text, resolving `loc:key` expectations.
    VisibleText,
    /// Assert selector states from a `{ "hover": true }` object on the queried control.
    State,
    /// Require a named emitted gameplay action.
    Action,
    /// Advance the deterministic UI clock by `value` milliseconds.
    Advance,
}

/// Default strongly typed model used by the bundled component catalogue.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct UiStoryFixture {
    /// Displayed or editable example text.
    pub text: String,
    /// Example boolean control state.
    pub checked: bool,
    /// Example numeric control state.
    pub value: f64,
}

impl Query {
    /// Find an accessible role, optionally qualified by an exact localized accessible name.
    pub fn role(role: &str, name: Option<&str>) -> Self {
        Self {
            by: QueryBy::Role,
            value: role.into(),
            name: name.map(str::to_owned),
        }
    }
    /// Find an exact localized accessible label.
    pub fn label(label: &str) -> Self {
        Self {
            by: QueryBy::Label,
            value: label.into(),
            name: None,
        }
    }
    /// Find exact currently displayed localized text.
    pub fn text(text: &str) -> Self {
        Self {
            by: QueryBy::Text,
            value: text.into(),
            name: None,
        }
    }
    /// Find an explicit test ID, or a stable authored ID when no test ID is present.
    pub fn test_id(id: &str) -> Self {
        Self {
            by: QueryBy::TestId,
            value: id.into(),
            name: None,
        }
    }
}
