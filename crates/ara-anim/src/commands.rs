//! Shared CLI/MCP/editor commands over the existing `AnimationCommand` API.
use crate::document::{AnimAsset, AnimationCommand};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_doc::RecordId;
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

fn invalid(message: impl ToString) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message.to_string())
}
fn check(cx: &EditCtx<'_, '_>) -> Result<(), Diagnostic> {
    if cx.read().doc().kind != "animation" {
        return Err(invalid("expected animation document"));
    }
    Ok(())
}
fn apply(
    cx: &mut EditCtx<'_, '_>,
    id: &str,
    command: AnimationCommand,
) -> Result<Value, Diagnostic> {
    check(cx)?;
    let record = RecordId::parse(id).map_err(invalid)?;
    if record.prefix() != "an" {
        return Err(invalid("expected an_ animation record ID"));
    }
    let edit = command
        .edit(cx.read().doc().id.clone(), record)
        .map_err(invalid)?;
    cx.apply_edit(edit)?;
    Ok(json!({"id":id}))
}

/// Create one animation asset (skeleton, clip, controller or procedural setup).
#[derive(Schema, Deserialize)]
#[ara(params, module = "animation")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationCreateParams {
    /// Stable record ID or batch symbol; generated when omitted.
    pub id: Option<String>,
    /// Typed animation source asset.
    pub asset: AnimAsset,
}
struct Create;
impl Command for Create {
    type Params = AnimationCreateParams;
    type Output = Value;
    const NAME: &'static str = "animation.create";
    const SUMMARY: &'static str = "Create a typed animation skeleton, clip or controller record.";
    fn examples() -> Vec<Value> {
        vec![json!({"id":"an_idle","asset":{"name":"Idle"}})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        check(cx)?;
        let id = cx.new_id(p.id.as_deref(), "an")?;
        apply(cx, id.as_str(), AnimationCommand::Create(p.asset))
    }
}

macro_rules! setter {
    ($command:ident, $params:ident, $name:literal, $summary:literal, $ty:ty, $variant:ident) => {
        #[doc = $summary]
        #[derive(Schema, Deserialize)]
        #[ara(params, module = "animation")]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $params {
            /// Existing animation record ID or a symbol created earlier in the batch.
            pub id: String,
            /// Replacement typed source data.
            pub value: $ty,
        }
        struct $command;
        impl Command for $command {
            type Params = $params;
            type Output = Value;
            const NAME: &'static str = $name;
            const SUMMARY: &'static str = $summary;
            fn examples() -> Vec<Value> {
                vec![json!({"id":"an_idle","value":{}})]
            }
            fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
                apply(cx, &p.id, AnimationCommand::$variant(p.value))
            }
        }
    };
}
setter!(
    SkeletonSet,
    AnimationSkeletonSetParams,
    "animation.skeleton.set",
    "Replace animation skeleton source.",
    crate::data::SkeletonData,
    SetSkeleton
);
setter!(
    ClipSet,
    AnimationClipSetParams,
    "animation.clip.set",
    "Replace animation clip keys.",
    crate::data::ClipData,
    SetClip
);
setter!(
    ControllerSet,
    AnimationControllerSetParams,
    "animation.controller.set",
    "Replace an animation controller graph.",
    crate::graph::ControllerData,
    SetController
);
setter!(
    RigSet,
    AnimationRigSetParams,
    "animation.rig.set",
    "Replace semantic humanoid bone bindings.",
    crate::retarget::RigMap,
    SetRig
);
setter!(
    IkSet,
    AnimationIkSetParams,
    "animation.ik.set",
    "Replace procedural animation contact chains.",
    crate::document::IkSetup,
    SetIk
);
setter!(
    DatabaseSet,
    AnimationMotionDatabaseSetParams,
    "animation.motionDatabase.set",
    "Replace pose-search database configuration.",
    crate::matching::MatchConfig,
    SetMotionDatabase
);
setter!(
    RagdollSet,
    AnimationRagdollSetParams,
    "animation.ragdoll.set",
    "Replace a physical animation ragdoll profile.",
    crate::physical::RagdollProfile,
    SetRagdoll
);

/// Edit one schema-addressed field of an animation record.
#[derive(Schema, Deserialize)]
#[ara(params, module = "animation")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationPropertySetParams {
    /// Animation record ID.
    pub id: String,
    /// Reflected field path, including nested graph and curve fields.
    pub path: String,
    /// Typed JSON replacement; null removes the stored value to its default.
    pub value: Value,
}
struct PropertySet;
impl Command for PropertySet {
    type Params = AnimationPropertySetParams;
    type Output = Value;
    const NAME: &'static str = "animation.property.set";
    const SUMMARY: &'static str = "Set or reset a schema-addressed animation field.";
    fn examples() -> Vec<Value> {
        vec![json!({"id":"an_idle","path":"name","value":"Rest"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        let path: FieldPath = p.path.parse().map_err(invalid)?;
        apply(
            cx,
            &p.id,
            AnimationCommand::SetField {
                path,
                value: (!p.value.is_null()).then_some(p.value),
            },
        )
    }
}

/// Register every typed animation authoring operation in the engine command catalog.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<Create>(db)
        .command::<SkeletonSet>(db)
        .command::<ClipSet>(db)
        .command::<ControllerSet>(db)
        .command::<RigSet>(db)
        .command::<IkSet>(db)
        .command::<DatabaseSet>(db)
        .command::<RagdollSet>(db)
        .command::<PropertySet>(db);
}
