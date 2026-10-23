//! Typed music document edits shared by the CLI, MCP and editor.
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_doc::RecordId;
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn invalid(message: impl ToString) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message.to_string())
}

fn set(
    cx: &mut EditCtx<'_, '_>,
    requested: Option<&str>,
    prefix: &str,
    value: &impl Serialize,
) -> Result<Value, Diagnostic> {
    if cx.read().doc().kind != "music" {
        return Err(invalid("expected music document"));
    }
    let record = ara_json::to_value(value)
        .map_err(invalid)?
        .as_object()
        .cloned()
        .ok_or_else(|| invalid("expected music record"))?;
    let id = if let Some(id) = requested.filter(|id| cx.read().doc().record(id).is_some()) {
        let id = RecordId::parse(id).map_err(invalid)?;
        if id.prefix() != prefix {
            return Err(invalid("record ID has the wrong music record prefix"));
        }
        for (key, value) in record {
            cx.set(&id, FieldPath::field(key), Some(value))?;
        }
        id
    } else {
        let id = cx.new_id(requested, prefix)?;
        cx.create(id.clone(), record)?;
        id
    };
    Ok(json!({"id":id}))
}

macro_rules! record_command {
    ($command:ident, $params:ident, $name:literal, $summary:literal, $ty:ty, $field:ident, $prefix:literal, $example:expr) => {
        #[doc = $summary]
        #[derive(Schema, Deserialize)]
        #[ara(params, module = "audio")]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $params {
            /// Existing record ID, stable new ID or batch symbol; generated when omitted.
            pub id: Option<String>,
            /// Complete typed music record; omitted fields take their reflected defaults.
            pub $field: $ty,
        }
        struct $command;
        impl Command for $command {
            type Params = $params;
            type Output = Value;
            const NAME: &'static str = $name;
            const SUMMARY: &'static str = $summary;
            fn examples() -> Vec<Value> {
                vec![$example]
            }
            fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
                set(cx, p.id.as_deref(), $prefix, &p.$field)
            }
        }
    };
}
record_command!(
    Segment,
    MusicSegmentSetParams,
    "music.segment.set",
    "Create or replace a music segment with stems and tempo map.",
    crate::SegmentDesc,
    segment,
    "sg",
    json!({"id":"sg_ambient","segment":{"name":"Ambient"}})
);
record_command!(
    Transition,
    MusicTransitionSetParams,
    "music.transition.set",
    "Create or replace a synchronized music transition.",
    crate::TransitionDesc,
    transition,
    "tn",
    json!({"transition":{"to":"Combat","sync":"bar"}})
);
record_command!(
    Stinger,
    MusicStingerSetParams,
    "music.stinger.set",
    "Create or replace a grid-aligned music stinger.",
    crate::StingerDesc,
    stinger,
    "st",
    json!({"stinger":{"name":"Discovery"}})
);

/// Music-wide starting segment, intensity, layer fade and deterministic seed.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MusicSettingsSetParams {
    /// Complete music settings, stored in the unique ms_settings record.
    pub settings: crate::MusicSettings,
}
struct Settings;
impl Command for Settings {
    type Params = MusicSettingsSetParams;
    type Output = Value;
    const NAME: &'static str = "music.settings.set";
    const SUMMARY: &'static str = "Set the music's initial segment, intensity, fade and seed.";
    fn examples() -> Vec<Value> {
        vec![json!({"settings":{"initial":"Ambient"}})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        set(cx, Some("ms_settings"), "ms", &p.settings)
    }
}

/// Registers typed segment, transition, stinger and settings commands.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<Segment>(db)
        .command::<Transition>(db)
        .command::<Stinger>(db)
        .command::<Settings>(db);
}
