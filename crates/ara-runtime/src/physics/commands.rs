//! Typed model-collider authoring; generation consumes the same cooked model as rendering.
use crate::components::MeshRenderer;
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_physics::{Collider, ColliderKind, MotionType, RigidBody};
use ara_schema::{AssetRef, EntityRef, FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

/// Geometry used by a model-derived collider.
#[derive(Default, Schema, Deserialize)]
#[ara(module = "physics")]
#[serde(rename_all = "camelCase")]
pub enum ModelColliderMode {
    /// Convex hull, usable by static, kinematic and dynamic bodies.
    #[default]
    Convex,
    /// Exact triangle surface, usable only by static bodies.
    Mesh,
}

/// Derive a collider from an imported model or the entity's MeshRenderer selection.
#[derive(Schema, Deserialize)]
#[ara(params, module = "physics")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColliderFromModelParams {
    /// Target entity, including prefab addresses.
    pub entity: EntityRef,
    /// Source import; omitted uses MeshRenderer.model.
    pub model: Option<AssetRef>,
    /// Source node name/index; omitted inherits MeshRenderer.node when model is omitted.
    pub node: Option<String>,
    /// Subtree-local primitive; omitted inherits MeshRenderer.primitive when model is omitted.
    pub primitive: Option<u32>,
    /// Convex hull by default, or a static triangle surface.
    #[ara(default = "convex")]
    #[serde(default)]
    pub mode: ModelColliderMode,
    /// Optional center in body-local metres, after visual scale is baked.
    pub center: Option<[f32; 3]>,
}

/// Adds a model-derived collider, retaining material settings and existing body motion.
pub struct ColliderFromModel;
impl Command for ColliderFromModel {
    type Params = ColliderFromModelParams;
    type Output = Value;
    const NAME: &'static str = "physics.collider.fromModel";
    const SUMMARY: &'static str =
        "Derive convex or static mesh collision from cooked model geometry.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_crate","mode":"convex"}),
            json!({"entity":"e_wall","mode":"mesh","center":[0,0.5,0]}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        let invalid = |message: String| Diagnostic::error(codes::INVALID_PARAMS, message);
        let target = ctx.read().target(&p.entity.0)?;
        let read = ctx.read();
        let entity = read
            .entity(&target)
            .ok_or_else(|| invalid("missing target entity".into()))?;
        if entity.record.contains_key("CharacterController")
            || entity.record.contains_key("Vehicle")
        {
            return Err(invalid(
                "a model collider requires a rigid body; remove the character/vehicle role first"
                    .into(),
            ));
        }
        let renderer: Option<MeshRenderer> = entity
            .record
            .get("MeshRenderer")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|e| invalid(e.to_string()))?;
        let inherit = p.model.is_none();
        let model = p
            .model
            .or_else(|| renderer.as_ref().and_then(|r| r.model.clone()))
            .ok_or_else(|| invalid("provide model or add a MeshRenderer.model first".into()))?;
        let (reference, _) = ara_doc::refs::resolve(&model.0, None, read.workspace().index())?;
        let model = AssetRef(reference);
        let existing: Option<Collider> = entity
            .record
            .get("Collider")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|e| invalid(e.to_string()))?;
        let body: Option<RigidBody> = entity
            .record
            .get("RigidBody")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|e| invalid(e.to_string()))?;
        let kind = match p.mode {
            ModelColliderMode::Convex => ColliderKind::ModelConvex,
            ModelColliderMode::Mesh => ColliderKind::ModelMesh,
        };
        if kind == ColliderKind::ModelMesh && body.is_some_and(|b| b.motion != MotionType::Static) {
            return Err(invalid(
                "model mesh collision requires a static RigidBody; use convex for moving bodies"
                    .into(),
            ));
        }
        let mut collider = existing.unwrap_or_default();
        collider.kind = kind;
        collider.asset = Some(model);
        collider.node = p.node.or_else(|| {
            inherit
                .then(|| renderer.as_ref().and_then(|r| r.node.clone()))
                .flatten()
        });
        collider.primitive = p.primitive.or_else(|| {
            inherit
                .then(|| renderer.as_ref().and_then(|r| r.primitive))
                .flatten()
        });
        if let Some(center) = p.center {
            collider.center = center;
        }
        if collider.center.iter().any(|v| !v.is_finite()) {
            return Err(invalid("collider center must be finite".into()));
        }
        let value = ara_json::to_value(&collider).map_err(|e| invalid(e.to_string()))?;
        ctx.set_entity(&target, &FieldPath::field("Collider"), Some(value.clone()))?;
        if body.is_none() {
            ctx.set_entity(
                &target,
                &FieldPath::field("RigidBody"),
                Some(
                    ara_json::to_value(&RigidBody::default())
                        .map_err(|e| invalid(e.to_string()))?,
                ),
            )?;
        }
        Ok(value)
    }
}

pub(crate) fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry.command::<ColliderFromModel>(db);
}
