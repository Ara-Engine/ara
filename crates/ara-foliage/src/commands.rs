//! Agent/editor command registration; every gesture is an attributed kernel txn.
use crate::{Exclusion, Header, Paint, Rule};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_doc::RecordId;
use ara_schema::{FieldPath, Schema, SchemaDb};
use ara_terrain::invalid;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Rule insertion/replacement parameters.
#[derive(Schema, Deserialize)]
#[ara(params, name = "FoliageRuleParams", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleParams {
    /// Optional existing ID or new symbolic ID.
    pub id: Option<String>,
    /// Seeded rule and placement filters.
    pub rule: Rule,
}
/// One manual brush stroke.
#[derive(Schema, Deserialize)]
#[ara(params, name = "FoliagePaintParams", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaintParams {
    /// Whole gesture, with all points in order.
    pub stroke: Paint,
}
/// Exclusion insertion parameters.
#[derive(Schema, Deserialize)]
#[ara(params, name = "FoliageExcludeParams", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcludeParams {
    /// Circular exclusion zone.
    pub zone: Exclusion,
}
/// Foliage source terrain.
#[derive(Schema, Deserialize)]
#[ara(params, name = "FoliageCreateParams", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateParams {
    /// Source terrain dependency.
    pub header: Header,
}
/// Stable record deletion for erasing manual strokes/rules/zones.
#[derive(Schema, Deserialize)]
#[ara(params, name = "FoliageEraseParams", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EraseParams {
    /// Existing paint, exclusion or unused rule record ID.
    pub id: String,
}
fn rec(v: &impl Serialize) -> Result<ara_doc::Record, Diagnostic> {
    match serde_json::to_value(v).map_err(|e| invalid(e.to_string()))? {
        Value::Object(v) => Ok(v),
        _ => Err(invalid("expected record")),
    }
}
fn check(ctx: &EditCtx<'_, '_>) -> Result<(), Diagnostic> {
    if ctx.read().doc().kind != "foliage" {
        Err(invalid("expected foliage document"))
    } else {
        Ok(())
    }
}
/// Registers foliage commands in the shared palette/CLI/MCP catalog.
pub fn register(reg: &mut Registry, db: &mut SchemaDb) {
    reg.command::<Create>(db)
        .command::<SetRule>(db)
        .command::<PaintCommand>(db)
        .command::<Exclude>(db)
        .command::<Erase>(db);
}
struct Create;
impl Command for Create {
    type Params = CreateParams;
    type Output = Value;
    const NAME: &'static str = "foliage.create";
    const SUMMARY: &'static str = "Create a foliage document linked to a terrain.";
    fn examples() -> Vec<Value> {
        vec![json!({"header":{"terrain":"@world.terrain#tr_world001"}})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: CreateParams) -> Result<Value, Diagnostic> {
        check(ctx)?;
        let id = ctx.new_id(Some("f_source"), "f")?;
        ctx.create(id, rec(&p.header)?)?;
        Ok(json!({"id":"f_source"}))
    }
}
struct SetRule;
impl Command for SetRule {
    type Params = RuleParams;
    type Output = Value;
    const NAME: &'static str = "foliage.rule.set";
    const SUMMARY: &'static str =
        "Add or update a seeded scatter rule; worker regeneration keeps stable identities.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"rule":{"name":"Grass","species":"grass","asset":"@grass.foliage#fo_grass001","seed":42,"cellSize":2,"density":0.2,"height":[0,500],"slope":[0,40],"channel":0,"minWeight":0.1,"noise":0.3,"scale":[0.8,1.2],"enabled":true}}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: RuleParams) -> Result<Value, Diagnostic> {
        check(ctx)?;
        p.rule.validate()?;
        if let Some(id) =
            p.id.as_deref()
                .filter(|id| ctx.read().doc().record(id).is_some())
        {
            let id = RecordId::parse(id).map_err(invalid)?;
            if id.prefix() != "r" {
                return Err(invalid("expected rule ID"));
            }
            for (k, v) in rec(&p.rule)? {
                ctx.set(&id, FieldPath::field(k), Some(v))?;
            }
            Ok(json!({"id":id}))
        } else {
            let id = ctx.new_id(p.id.as_deref(), "r")?;
            ctx.create(id.clone(), rec(&p.rule)?)?;
            Ok(json!({"id":id}))
        }
    }
}
struct PaintCommand;
impl Command for PaintCommand {
    type Params = PaintParams;
    type Output = Value;
    const NAME: &'static str = "foliage.paint";
    const SUMMARY: &'static str = "Record one complete manual foliage brush gesture.";
    fn examples() -> Vec<Value> {
        vec![json!({"stroke":{"rule":"r_grass","seed":42,"points":[[0,0],[1,2]]}})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: PaintParams) -> Result<Value, Diagnostic> {
        check(ctx)?;
        if !p.stroke.rule.starts_with("r_")
            || ctx.read().doc().record(&p.stroke.rule).is_none()
            || p.stroke.points.is_empty()
            || p.stroke.points.len() > 4096
            || p.stroke
                .points
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || v.abs() > 1e7)
        {
            return Err(invalid("invalid rule or manual stroke points"));
        }
        let id = ctx.new_id(None, "p")?;
        ctx.create(id.clone(), rec(&p.stroke)?)?;
        Ok(json!({"id":id}))
    }
}
struct Exclude;
impl Command for Exclude {
    type Params = ExcludeParams;
    type Output = Value;
    const NAME: &'static str = "foliage.exclude";
    const SUMMARY: &'static str = "Exclude manual and procedural foliage from a circular zone.";
    fn examples() -> Vec<Value> {
        vec![json!({"zone":{"center":[10,20],"radius":4}})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: ExcludeParams) -> Result<Value, Diagnostic> {
        check(ctx)?;
        if !(0.0..=65536.).contains(&p.zone.radius) || p.zone.center.iter().any(|v| !v.is_finite())
        {
            return Err(invalid("invalid exclusion zone"));
        }
        let id = ctx.new_id(None, "x")?;
        ctx.create(id.clone(), rec(&p.zone)?)?;
        Ok(json!({"id":id}))
    }
}
struct Erase;
impl Command for Erase {
    type Params = EraseParams;
    type Output = Value;
    const NAME: &'static str = "foliage.erase";
    const SUMMARY: &'static str =
        "Erase a paint gesture, exclusion or unused scatter rule with undo.";
    fn examples() -> Vec<Value> {
        vec![json!({"id":"p_brush"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: EraseParams) -> Result<Value, Diagnostic> {
        check(ctx)?;
        let id = RecordId::parse(&p.id).map_err(invalid)?;
        if !["p", "r", "x"].contains(&id.prefix()) {
            return Err(invalid(
                "only paint strokes, exclusions and rules can be erased",
            ));
        }
        if id.prefix() == "r"
            && ctx
                .read()
                .doc()
                .records()
                .values()
                .any(|r| r.get("rule").and_then(Value::as_str) == Some(&p.id))
        {
            return Err(invalid(
                "erase dependent manual strokes before removing this rule",
            ));
        }
        ctx.delete(&id)?;
        Ok(json!({"id":id}))
    }
}
