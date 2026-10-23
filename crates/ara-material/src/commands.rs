//! Authoring may retain type errors for interactive correction. Commands report
//! diagnostics; only checked ASTs can generate/publish artifacts. All edits are ops.

use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_graph::{Graph, commands::apply_snapshot};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Binding, Expr, Material, check, codes, dsl};

/// Replaces the text view, retaining layout for surviving semantic IDs.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialTextParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextParams {
    /// Complete bounded material DSL.
    pub text: String,
}
/// Applies an edited graph projection as the same material AST.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialGraphParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphParams {
    /// Canonical graph JSON produced by ara-json.
    pub json: String,
}
/// Changes a node's declared type (also used by type annotation fixes).
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialTypeParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypeParams {
    /// Stable expression/parameter ID.
    pub node: String,
    /// Exact material type name.
    pub ty: String,
}
/// Replaces an expression with a typed constant.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialLiteralParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiteralParams {
    /// Stable expression ID.
    pub node: String,
    /// New literal lanes.
    pub values: Vec<f64>,
}
/// Identifies a material parameter.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialParameterParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterParams {
    /// Stable parameter ID.
    pub node: String,
}
/// Identifies a domain output field.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialOutputParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputParams {
    /// Domain field name.
    pub field: String,
}
/// Sets an explicit material specialization budget.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialBudgetParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BudgetParams {
    /// Maximum static combinations.
    pub budget: u32,
}
/// Rewires an AST argument or output, preserving the destination edge's identity.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialConnectParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectParams {
    /// Source semantic node ID.
    pub source: String,
    /// Consumer node ID, or n_output for a domain field.
    pub node: String,
    /// in0/in1/... for function arguments; domain field for n_output.
    pub port: String,
}

fn load(ctx: &EditCtx<'_, '_>) -> Result<Material, Diagnostic> {
    let g = Graph::from_document(ctx.read().doc())?;
    Material::from_graph(&g).map_err(first)
}
fn first(d: Vec<Diagnostic>) -> Diagnostic {
    d.into_iter()
        .next()
        .unwrap_or_else(|| Diagnostic::error(codes::SYNTAX, "invalid material"))
}
fn save(ctx: &mut EditCtx<'_, '_>, m: &Material) -> Result<Value, Diagnostic> {
    let editor = Graph::from_document(ctx.read().doc())?.editor;
    apply_snapshot(ctx, &m.to_graph(editor)?)?;
    Ok(json!({"diagnostics":check(m).err().unwrap_or_default()}))
}
fn missing() -> Diagnostic {
    Diagnostic::error(codes::REFERENCE, "material node or field does not exist")
}

macro_rules! command {
    ($name:ident,$params:ty,$key:literal,$doc:literal,$example:expr,|$ctx:ident,$p:ident|$body:block)=>{
        #[doc=$doc]
        pub struct $name;
        impl Command for $name {
            type Params=$params;type Output=Value;
            const NAME:&'static str=$key;const SUMMARY:&'static str=$doc;
            fn examples()->Vec<Value>{vec![$example]}
            fn run($ctx:&mut EditCtx<'_, '_>,$p:Self::Params)->Result<Value,Diagnostic>$body
        }
    }
}
command!(
    ApplyText,
    TextParams,
    "material.text.apply",
    "Apply the text view as undoable material ops.",
    json!({"text":"material gr_demo Demo : unlit budget 1 { let n_a color : ColorLinear = [1, 1, 1]; output color = color; }"}),
    |ctx, p| {
        let m = dsl::parse(&p.text).map_err(first)?;
        save(ctx, &m)
    }
);
command!(
    ApplyGraph,
    GraphParams,
    "material.graph.apply",
    "Apply an edited graph projection.",
    json!({"json":"{}"}),
    |ctx, p| {
        let graph = Graph::read(&p.json, ctx.read().schema()).map_err(first)?;
        let m = Material::from_graph(&graph).map_err(first)?;
        let diags = check(&m).err().unwrap_or_default();
        apply_snapshot(ctx, &graph)?;
        Ok(json!({"diagnostics":diags}))
    }
);
command!(
    SetType,
    TypeParams,
    "material.node.type",
    "Change a material type annotation.",
    json!({"node":"n_a","ty":"float"}),
    |ctx, p| {
        let mut m = load(ctx)?;
        let ty =
            p.ty.parse()
                .map_err(|e| Diagnostic::error(codes::TYPE, e))?;
        if let Some(b) = m.bindings.get_mut(&p.node) {
            b.ty = ty;
        } else if let Some(b) = m.parameters.get_mut(&p.node) {
            b.ty = ty;
        } else {
            return Err(missing());
        }
        save(ctx, &m)
    }
);
command!(
    SetLiteral,
    LiteralParams,
    "material.node.literal",
    "Replace an expression with a typed literal.",
    json!({"node":"n_a","values":[0]}),
    |ctx, p| {
        let mut m = load(ctx)?;
        m.bindings.get_mut(&p.node).ok_or_else(missing)?.expr = Expr::Literal { values: p.values };
        save(ctx, &m)
    }
);
command!(
    ResetParameter,
    ParameterParams,
    "material.parameter.reset",
    "Reset invalid parameter values and static choices.",
    json!({"node":"n_a"}),
    |ctx, p| {
        let mut m = load(ctx)?;
        let b = m.parameters.get_mut(&p.node).ok_or_else(missing)?;
        b.default = vec![0.0; b.ty.lanes()];
        b.variants.clear();
        save(ctx, &m)
    }
);
command!(
    DefaultOutput,
    OutputParams,
    "material.output.default",
    "Reset an output to a typed zero value.",
    json!({"field":"roughness"}),
    |ctx, p| {
        let mut m = load(ctx)?;
        let ty = *m
            .domain
            .outputs()
            .get(p.field.as_str())
            .ok_or_else(missing)?;
        let id = ctx.new_id(None, "n")?.to_string();
        let name = format!("default_{}", &id[2..]);
        m.bindings.insert(
            id.clone(),
            Binding {
                name,
                ty,
                expr: Expr::Literal {
                    values: vec![0.0; ty.lanes()],
                },
            },
        );
        m.outputs.insert(p.field, id);
        save(ctx, &m)
    }
);
command!(
    SetBudget,
    BudgetParams,
    "material.budget.set",
    "Set an explicit static variant budget.",
    json!({"budget":16}),
    |ctx, p| {
        let mut m = load(ctx)?;
        m.variant_budget = p.budget;
        save(ctx, &m)
    }
);
command!(
    Connect,
    ConnectParams,
    "material.connect",
    "Rewire an argument or output in the shared material AST.",
    json!({"source":"n_a","node":"n_b","port":"in0"}),
    |ctx, p| {
        let mut m = load(ctx)?;
        if m.node_type(&p.source).is_none() {
            return Err(missing());
        }
        if p.node == "n_output" {
            if !m.domain.outputs().contains_key(p.port.as_str()) {
                return Err(missing());
            }
            m.outputs.insert(p.port, p.source);
        } else {
            let index = p
                .port
                .strip_prefix("in")
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or_else(missing)?;
            let b = m.bindings.get_mut(&p.node).ok_or_else(missing)?;
            let Expr::Call { args, .. } = &mut b.expr else {
                return Err(missing());
            };
            *args.get_mut(index).ok_or_else(missing)? = p.source;
        }
        // Acyclicity is structural, so reject cycles before applying graph ops.
        let graph = m.to_graph(Default::default())?;
        graph.topological_order()?;
        save(ctx, &m)
    }
);

/// Registers material text/graph edits and all machine-applicable fix commands.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    crate::instance_asset::register_commands(registry, db);
    registry
        .command::<ApplyText>(db)
        .command::<ApplyGraph>(db)
        .command::<SetType>(db)
        .command::<SetLiteral>(db)
        .command::<ResetParameter>(db)
        .command::<DefaultOutput>(db)
        .command::<SetBudget>(db)
        .command::<Connect>(db);
}
