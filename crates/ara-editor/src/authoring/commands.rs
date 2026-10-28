//! Domain edits lower to shared graph/animation ops; no widget writes a document.
mod resource;
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_graph::Graph;
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
/// Initial typed domain source for an empty graph document.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorGraphInitializeParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InitializeParams {
    /// material, vfx or generic graph domain.
    domain: String,
    /// Optional domain text; absent creates a useful editable starter.
    source: Option<String>,
}
struct Initialize;
impl Command for Initialize {
    type Params = InitializeParams;
    type Output = Value;
    const NAME: &'static str = "editor.graph.initialize";
    const SUMMARY: &'static str = "Initialize a graph asset through undoable record ops.";
    fn examples() -> Vec<Value> {
        vec![json!({"domain":"material"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: InitializeParams) -> Result<Value, Diagnostic> {
        if cx.read().doc().kind != "graph" || !cx.read().doc().records().is_empty() {
            return Err(crate::invalid("initialization requires an empty graph"));
        }
        let id = cx.read().doc().id.clone();
        let graph=match p.domain.as_str(){
            "material"=>ara_material::dsl::parse(p.source.as_deref().unwrap_or("material Paint : surface budget 1 { param color : ColorLinear = linear(0.18, 0.42, 0.72); param roughness : float = 0.35; let metallic : float = 0.3; output baseColor = color; output roughness = roughness; output metallic = metallic; }"),id.as_str()).map_err(first)?.to_graph(Default::default())?,
            "vfx"=>p.source.map(|s|ara_vfx::Effect::parse(&s)).transpose()?.unwrap_or_else(ara_vfx::samples::campfire).to_graph(id,Default::default())?,
            _=>Graph::new(id,p.domain),
        };
        ara_graph::commands::apply_snapshot(cx, &graph)?;
        Ok(Value::Null)
    }
}
pub(super) fn first(diags: Vec<Diagnostic>) -> Diagnostic {
    diags
        .into_iter()
        .next()
        .unwrap_or_else(|| crate::invalid("domain validation failed"))
}
/// Add a documented material operation, creating typed default argument nodes.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorMaterialAddParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MaterialAddParams {
    /// Registered operation key, including material.literal and material.parameter.
    kind: String,
    /// Canvas coordinate for the new operation.
    position: [f32; 2],
}
struct MaterialAdd;
impl Command for MaterialAdd {
    type Params = MaterialAddParams;
    type Output = Value;
    const NAME: &'static str = "editor.material.node.add";
    const SUMMARY: &'static str = "Add a typed material node with valid default inputs.";
    fn examples() -> Vec<Value> {
        vec![json!({"kind":"material.add","position":[200,120]})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: MaterialAddParams) -> Result<Value, Diagnostic> {
        use ara_material::{Binding, Expr, Parameter, ParameterValue, Type};
        let graph = Graph::from_document(cx.read().doc())?;
        let mut material = ara_material::Material::from_graph(&graph).map_err(first)?;
        let op = p.kind.strip_prefix("material.").unwrap_or(&p.kind);
        let id = cx.new_id(None, "n")?.to_string();
        let name = format!("value_{}", id.trim_start_matches("n_"));
        let order = material
            .bindings
            .values()
            .map(|b| b.order)
            .chain(material.parameters.values().map(|b| b.order))
            .max()
            .unwrap_or(0)
            + 1;
        let mut layout = graph.editor;
        if op == "parameter" {
            material.parameters.insert(
                id.clone(),
                Parameter {
                    name,
                    order,
                    ty: Type::Float,
                    default: ParameterValue::Numeric(vec![0.5]),
                    is_static: false,
                },
            );
        } else {
            let (ty, expr) = if op == "literal" {
                (Type::Float, Expr::Literal { values: vec![0.5] })
            } else {
                let mut signature = None;
                // Registry signatures define the valid arity/types; defaults are independent nodes.
                for arity in 0..=4 {
                    if let Some(found) = signature_for(op, arity, &mut Vec::new()) {
                        signature = Some(found);
                        break;
                    }
                }
                let (output,types)=signature.ok_or_else(||crate::invalid("This operation needs an explicit typed expression; use the synchronized DSL pane."))?;
                let mut args = Vec::new();
                for (index, ty) in types.into_iter().enumerate() {
                    let arg = cx.new_id(None, "n")?.to_string();
                    let arg_name = format!("{name}Input{index}");
                    if ty.is_texture() {
                        material.parameters.insert(
                            arg.clone(),
                            Parameter {
                                name: arg_name,
                                order: order + index as u32,
                                ty,
                                default: ParameterValue::zero(ty),
                                is_static: false,
                            },
                        );
                    } else {
                        material.bindings.insert(
                            arg.clone(),
                            Binding {
                                name: arg_name,
                                order: order + index as u32,
                                ty,
                                expr: Expr::Literal {
                                    values: vec![
                                        if ty == Type::Bool { 0. } else { 0.5 };
                                        ty.lanes()
                                    ],
                                },
                            },
                        );
                    }
                    layout.positions.insert(
                        arg.clone(),
                        [p.position[0] - 256., p.position[1] + index as f32 * 136.],
                    );
                    args.push(arg);
                }
                (
                    output,
                    Expr::Call {
                        op: op.into(),
                        args,
                    },
                )
            };
            material.bindings.insert(
                id.clone(),
                Binding {
                    name,
                    order: order + 8,
                    ty,
                    expr,
                },
            );
        }
        layout.positions.insert(id.clone(), p.position);
        let graph = material.to_graph(layout)?;
        ara_graph::commands::apply_snapshot(cx, &graph)?;
        Ok(json!({"id":id,"diagnostics":ara_material::check(&material).err().unwrap_or_default()}))
    }
}
/// Complete controller edit from states, transitions or a blend-space parameter panel.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorAnimationParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnimationParams {
    /// Source record; the conventional controller record is an_controller.
    record: String,
    /// Typed controller data; semantic references are checked before persistence.
    controller: ara_anim::graph::ControllerData,
}
struct AnimationSet;
impl Command for AnimationSet {
    type Params = AnimationParams;
    type Output = Value;
    const NAME: &'static str = "editor.animation.controller.set";
    const SUMMARY: &'static str =
        "Edit animation states, transitions and blend spaces as one transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"record":"an_controller","controller":{}})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: AnimationParams) -> Result<Value, Diagnostic> {
        if cx.read().doc().kind != "animation" {
            return Err(crate::invalid("expected an animation document"));
        }
        super::animation::validate(&p.controller)?;
        for state in p.controller.states.values() {
            if !p.controller.nodes.contains_key(&state.node) {
                return Err(crate::invalid("state references a missing pose node"));
            }
        }
        for transition in &p.controller.transitions {
            if !p.controller.states.contains_key(&transition.from)
                || !p.controller.states.contains_key(&transition.to)
            {
                return Err(crate::invalid("transition references a missing state"));
            }
        }
        for node in p.controller.nodes.values() {
            for sample in &node.samples {
                if !p.controller.nodes.contains_key(&sample.node)
                    || !sample.x.is_finite()
                    || !sample.y.is_finite()
                {
                    return Err(crate::invalid("invalid blend-space sample"));
                }
            }
        }
        let id = ara_doc::RecordId::parse(&p.record).map_err(crate::invalid)?;
        let controller =
            ara_json::to_value(&p.controller).map_err(|e| crate::invalid(e.to_string()))?;
        if cx.read().doc().record(&p.record).is_some() {
            cx.set(&id, FieldPath::field("controller"), Some(controller))?;
        } else {
            cx.create(
                id,
                json!({"name":"Controller","controller":controller})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| crate::invalid("controller record"))?,
            )?;
        }
        Ok(Value::Null)
    }
}
/// Replace a module's typed settings while retaining its name and stack position.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorVfxModuleParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModuleParams {
    /// Emitter name.
    emitter: String,
    /// Stage name.
    stage: String,
    /// Persistent module name.
    name: String,
    /// Typed module source.
    module: Value,
}
struct ModuleSet;
impl Command for ModuleSet {
    type Params = ModuleParams;
    type Output = Value;
    const NAME: &'static str = "editor.vfx.module.set";
    const SUMMARY: &'static str = "Set typed module parameters without changing stack order.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"emitter":"flame","stage":"update","name":"drag","module":{"type":"drag","coefficient":0.2}}),
        ]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: ModuleParams) -> Result<Value, Diagnostic> {
        let graph = Graph::from_document(cx.read().doc())?;
        let mut effect = ara_vfx::Effect::from_graph(&graph)?;
        let emitter = effect
            .emitters
            .iter_mut()
            .find(|e| e.name == p.emitter)
            .ok_or_else(|| crate::invalid("emitter missing"))?;
        let module = emitter
            .stacks
            .get_mut(
                serde_json::from_value(json!(p.stage))
                    .map_err(|e| crate::invalid(e.to_string()))?,
            )
            .iter_mut()
            .find(|m| m.name == p.name)
            .ok_or_else(|| crate::invalid("module missing"))?;
        module.module =
            serde_json::from_value(p.module).map_err(|e| crate::invalid(e.to_string()))?;
        effect.resolve(&Default::default())?;
        ara_graph::commands::apply_snapshot(cx, &effect.to_graph(graph.id, graph.editor)?)?;
        Ok(Value::Null)
    }
}
pub(super) fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<resource::Create>(db)
        .command::<Initialize>(db)
        .command::<MaterialAdd>(db)
        .command::<AnimationSet>(db)
        .command::<ModuleSet>(db);
    registry.command::<super::material_fields::Set>(db);
}

fn signature_for(
    op: &str,
    arity: usize,
    types: &mut Vec<ara_material::Type>,
) -> Option<(ara_material::Type, Vec<ara_material::Type>)> {
    if types.len() == arity {
        return ara_material::library::infer(op, types).map(|output| (output, types.clone()));
    }
    for ty in ara_material::Type::ALL {
        types.push(*ty);
        if let Some(found) = signature_for(op, arity, types) {
            return Some(found);
        }
        types.pop();
    }
    None
}
