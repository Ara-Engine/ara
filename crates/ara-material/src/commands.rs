//! Authoring may retain type errors for interactive correction. Commands report
//! diagnostics; only checked ASTs can generate/publish artifacts. All edits are ops.

use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_graph::{Graph, commands::apply_snapshot};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Binding, Expr, Material, ParameterValue, check, codes, dsl};

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

/// Renames an authored value while retaining its graph identity and layout.
#[derive(Schema, Deserialize)]
#[ara(params, module = "material", name = "MaterialRenameParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameParams {
    /// Existing authored name.
    pub from: String,
    /// New unique authored name.
    pub to: String,
    /// Diagnostic-only recovery identity after a plain-text delete/create rename.
    pub restore_id: Option<String>,
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
    persist(ctx, m, editor, Vec::new())
}
fn persist(
    ctx: &mut EditCtx<'_, '_>,
    m: &Material,
    editor: ara_graph::Editor,
    mut diagnostics: Vec<Diagnostic>,
) -> Result<Value, Diagnostic> {
    let mut m = m.clone();
    m.normalize_order()?;
    match m.resolve_assets(ctx.read().workspace().index()) {
        Ok(notes) => diagnostics.extend(notes),
        Err(errors) => {
            for diagnostic in &errors {
                if let Some(id) = &diagnostic.location.record
                    && let Some(parameter) = m.parameters.get_mut(id)
                {
                    parameter.default = ParameterValue::zero(parameter.ty);
                }
            }
            let replacement = dsl::print(&m)?;
            let mut diagnostic = first(errors);
            diagnostic.fixes.clear();
            return Err(dsl::source_fix(diagnostic, replacement));
        }
    }
    diagnostics.extend(check(&m).err().unwrap_or_default());
    apply_snapshot(ctx, &m.to_graph(editor)?)?;
    Ok(json!({"diagnostics":diagnostics}))
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
    json!({"text":"material Demo : unlit budget 1 { let color : ColorLinear = linear(1, 1, 1); output color = color; }"}),
    |ctx, p| {
        let graph = Graph::from_document(ctx.read().doc())?;
        let parsed = dsl::reconcile_graph(&p.text, &graph).map_err(first)?;
        persist(ctx, &parsed.material, graph.editor, parsed.diagnostics)
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
        persist(ctx, &m, graph.editor, Vec::new())
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
        b.default = ParameterValue::zero(b.ty);
        if b.ty != crate::Type::Bool {
            b.is_static = false;
        }
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
                order: m.bindings.len() as u32,
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

command!(
    Rename,
    RenameParams,
    "material.rename",
    "Rename a value while retaining its graph identity and layout.",
    json!({"from":"paint","to":"basePaint"}),
    |ctx, p| {
        if !crate::check::value_name(&p.to) {
            return Err(Diagnostic::error(
                codes::SYNTAX,
                "new name must be an identifier",
            ));
        }
        let mut m = load(ctx)?;
        let id = if let Some(id) = m.node_id(&p.from) {
            if p.from != p.to && m.node_id(&p.to).is_some() {
                return Err(Diagnostic::error(
                    codes::SYNTAX,
                    "new material name already exists",
                ));
            }
            id.to_owned()
        } else {
            let current = m.node_id(&p.to).ok_or_else(missing)?.to_owned();
            let restored = p.restore_id.ok_or_else(missing)?;
            if ara_schema::grammar::split_id(&restored).is_none_or(|(prefix, _)| prefix != "n")
                || m.node_type(&restored).is_some()
            {
                return Err(Diagnostic::error(
                    codes::SYNTAX,
                    "recovery identity is invalid or already occupied",
                ));
            }
            if let Some(value) = m.parameters.remove(&current) {
                m.parameters.insert(restored.clone(), value);
            } else if let Some(value) = m.bindings.remove(&current) {
                m.bindings.insert(restored.clone(), value);
            }
            for binding in m.bindings.values_mut() {
                if let Expr::Call { args, .. } = &mut binding.expr {
                    for arg in args {
                        if *arg == current {
                            *arg = restored.clone();
                        }
                    }
                }
            }
            for output in m.outputs.values_mut() {
                if *output == current {
                    *output = restored.clone();
                }
            }
            restored
        };
        if let Some(value) = m.parameters.get_mut(&id) {
            value.name = p.to;
        } else if let Some(value) = m.bindings.get_mut(&id) {
            value.name = p.to;
        }
        save(ctx, &m)
    }
);

/// Registers material text/graph edits, renames and machine-applicable fixes.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    crate::create::register(registry);
    crate::instance_asset::register_commands(registry, db);
    registry
        .command::<ApplyText>(db)
        .command::<ApplyGraph>(db)
        .command::<SetType>(db)
        .command::<SetLiteral>(db)
        .command::<ResetParameter>(db)
        .command::<DefaultOutput>(db)
        .command::<SetBudget>(db)
        .command::<Connect>(db)
        .command::<Rename>(db);
}
