//! Typed physics authoring, including replaceable tile-derived collider entities.
use crate::{Body2d, BodyKind2d, Character2d, Collider2d, Shape2d, solver};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_schema::{EntityRef, FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn invalid(e: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(ara_diag::Code(3602), e.to_string())
}
/// Complete body/collider/controller authoring update.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Physics2dSetParams {
    /// Target entity.
    pub entity: EntityRef,
    /// Authoritative body pose and integration policy.
    pub body: Body2d,
    /// Convex shape and physical material.
    pub collider: Collider2d,
    /// Optional controller; absence removes an existing controller.
    pub character: Option<Character2d>,
}
/// Sets the complete typed physical body in one undoable transaction.
pub struct SetPhysics;
impl Command for SetPhysics {
    type Params = Physics2dSetParams;
    type Output = ();
    const NAME: &'static str = "physics2d.set";
    const SUMMARY: &'static str = "Set a validated 2D rigid body, collider and optional character.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_body1","body":{},"collider":{}})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<(), Diagnostic> {
        solver::validate(&p.body, &p.collider, p.character.as_ref()).map_err(invalid)?;
        let target = ctx.read().target(&p.entity.0)?;
        ctx.set_entity(
            &target,
            &FieldPath::field("Body2d"),
            Some(serde_json::to_value(p.body).map_err(invalid)?),
        )?;
        ctx.set_entity(
            &target,
            &FieldPath::field("Collider2d"),
            Some(serde_json::to_value(p.collider).map_err(invalid)?),
        )?;
        if p.character.is_some() || ctx.read().has_component(&target, "Character2d") {
            ctx.set_entity(
                &target,
                &FieldPath::field("Character2d"),
                p.character
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(invalid)?,
            )?;
        }
        Ok(())
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
        if !matches!(target, ara_cmd::Target::Plain(_))
            || p.origin.iter().chain(basis.iter()).any(|v| !v.is_finite())
            || (basis[0] * basis[3] - basis[1] * basis[2]).abs() < 1e-8
        {
            return Err(invalid(
                "collider generation requires a plain entity and finite origin",
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
                shape: Shape2d::Polygon,
                points,
                ..Default::default()
            };
            solver::validate(&Body2d::default(), &collider, None).map_err(invalid)?;
            let value = json!({"parent":p.entity.0,"name":format!("{} collision",hull.layer),"Body2d":Body2d { kind:BodyKind2d::Fixed, position:p.origin,..Default::default() },"Collider2d":collider,"TileColliderSource2d":{"owner":p.entity.0}});
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
