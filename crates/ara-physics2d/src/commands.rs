//! Typed physics authoring, including replaceable tile-derived collider entities.
use crate::{
    CharacterController2d, Collider2d, ColliderKind2d, MotionType2d, PhysicsPose2d, RigidBody2d,
    Velocity2d, shape,
};
use ara_cmd::{Command, EditCtx, Registry, Target};
use ara_diag::Diagnostic;
use ara_schema::{EntityRef, FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn invalid(e: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(ara_diag::Code(3602), e.to_string())
}
fn set(
    ctx: &mut EditCtx<'_, '_>,
    target: &Target,
    component: &str,
    value: Option<impl Serialize>,
) -> Result<(), Diagnostic> {
    if value.is_none() && !ctx.read().has_component(target, component) {
        return Ok(());
    }
    let value = value
        .map(serde_json::to_value)
        .transpose()
        .map_err(invalid)?;
    ctx.set_entity(target, &FieldPath::field(component), value)
}

/// A complete rigid body or character, validated as one unit.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Physics2dSetParams {
    /// Target entity, including prefab addresses.
    pub entity: EntityRef,
    /// World-space starting pose.
    pub pose: PhysicsPose2d,
    /// Rigid body settings; requires `collider` and excludes `character`.
    pub body: Option<RigidBody2d>,
    /// Collision geometry and material for `body`.
    pub collider: Option<Collider2d>,
    /// Capsule character controller; excludes `body` and `collider`.
    pub character: Option<CharacterController2d>,
}
/// Replaces an entity's physical role in one undoable transaction, removing the
/// components of the role it no longer has.
pub struct SetPhysics;
impl Command for SetPhysics {
    type Params = Physics2dSetParams;
    type Output = ();
    const NAME: &'static str = "physics2d.set";
    const SUMMARY: &'static str =
        "Set a validated 2D rigid body (body + collider) or capsule character, with its pose.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_crate1","pose":{"position":[2,1]},"body":{"motion":"dynamic"},"collider":{"halfExtents":[0.5,0.5]}}),
            json!({"entity":"e_hero1","pose":{"position":[0,1]},"character":{"height":1.4}}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<(), Diagnostic> {
        shape::validate_state(&p.pose, &Velocity2d::default()).map_err(invalid)?;
        match (&p.body, &p.collider, &p.character) {
            (Some(body), Some(collider), None) => {
                shape::validate_body(body).map_err(invalid)?;
                shape::validate_collider(collider, body).map_err(invalid)?;
            }
            (None, None, Some(character)) => {
                shape::validate_character(character).map_err(invalid)?
            }
            _ => {
                return Err(invalid(
                    "provide either body and collider, or character, but not both",
                ));
            }
        }
        let target = ctx.read().target(&p.entity.0)?;
        set(ctx, &target, "PhysicsPose2d", Some(p.pose))?;
        set(ctx, &target, "RigidBody2d", p.body)?;
        set(ctx, &target, "Collider2d", p.collider)?;
        set(ctx, &target, "CharacterController2d", p.character)
    }
}

/// Ownership marker for generated colliders, separate from gameplay body state.
#[derive(Debug, Clone, Schema, Serialize, Deserialize, ara_sim::bevy_ecs::prelude::Component)]
#[ara(component, module = "physics2d")]
pub struct TileColliderSource2d {
    /// Map entity whose tiles generated this collider.
    pub owner: EntityRef,
}
/// Tile collider generation request.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileCollidersParams {
    /// Plain tilemap entity (prefab generation belongs in its source document).
    pub entity: EntityRef,
    /// World-space map offset in metres.
    pub origin: [f32; 2],
    /// Optional row-major map basis, including hierarchy rotation/scale/shear.
    pub basis: Option<[f32; 4]>,
}
/// Regenerates owned static hulls through ordinary create/delete ops.
pub struct GenerateTileColliders;
impl Command for GenerateTileColliders {
    type Params = TileCollidersParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.colliders.generate";
    const SUMMARY: &'static str = "Generate merged static tile colliders as undoable entities.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_map1","origin":[0,0]})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        let target = ctx.read().target(&p.entity.0)?;
        let basis = p.basis.unwrap_or([1., 0., 0., 1.]);
        if !matches!(target, Target::Plain(_))
            || p.origin.iter().chain(basis.iter()).any(|v| !v.is_finite())
            || (basis[0] * basis[3] - basis[1] * basis[2]).abs() < 1e-8
        {
            return Err(invalid(
                "collider generation requires a plain entity, finite origin and invertible basis",
            ));
        }
        let record = ctx
            .read()
            .entity(&target)
            .ok_or_else(|| invalid("missing map"))?
            .record;
        let map: ara_2d::Tilemap = serde_json::from_value(
            record
                .get("Tilemap")
                .ok_or_else(|| invalid("missing Tilemap"))?
                .clone(),
        )
        .map_err(invalid)?;
        let hulls = ara_2d::tilemap::colliders(&map).map_err(invalid)?;
        let old: Vec<_> = ctx
            .read()
            .doc()
            .records()
            .iter()
            .filter(|(_, r)| {
                r.get("TileColliderSource2d")
                    .and_then(|s| s.get("owner"))
                    .and_then(Value::as_str)
                    == Some(&p.entity.0)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in old {
            ctx.delete(&id)?;
        }
        let body = RigidBody2d {
            motion: MotionType2d::Static,
            ..Default::default()
        };
        for hull in &hulls {
            let id = ctx.new_id(None, "e")?;
            let points: Vec<_> = hull
                .vertices
                .iter()
                .map(|v| {
                    [
                        basis[0] * v[0] + basis[1] * v[1],
                        basis[2] * v[0] + basis[3] * v[1],
                    ]
                })
                .collect();
            let collider = Collider2d {
                kind: ColliderKind2d::Polygon,
                points,
                ..Default::default()
            };
            shape::validate_collider(&collider, &body).map_err(invalid)?;
            let value = json!({
                "parent": p.entity.0,
                "name": format!("{} collision", hull.layer),
                "PhysicsPose2d": PhysicsPose2d { position: p.origin, rotation: 0. },
                "RigidBody2d": body,
                "Collider2d": collider,
                "TileColliderSource2d": {"owner": p.entity.0},
            });
            ctx.create(id, value.as_object().expect("object literal").to_owned())?;
        }
        Ok(hulls.len() as u32)
    }
}
/// Installs the physics authoring API in Engine Protocol/MCP reflection.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    crate::register(db);
    db.register::<TileColliderSource2d>();
    registry.command::<SetPhysics>(db);
    registry.command::<GenerateTileColliders>(db);
}
