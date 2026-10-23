//! Typed, undoable audio-graph edits. Every command loads the graph document, edits the
//! AST by **name** (agents never handle IDs) and reconciles the projection into kernel
//! ops with `ara_graph::commands::apply_snapshot`. Results include the canonical text and
//! any check diagnostics: authoring may keep errors while they are being fixed.

use crate::{
    ast::{AudioGraph, Item, Node, ParamKind, Value, mint_id},
    check::check,
    codes, dsl,
    library::Op,
};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_graph::{Graph, commands::apply_snapshot};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

/// Replaces the graph with edited `.audiograph` text; surviving names keep their IDs.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio", name = "AudioGraphTextParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextParams {
    /// Complete `.audiograph` source.
    pub text: String,
}

/// Renames a parameter or node, keeping its identity, wiring and canvas position.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio", name = "AudioGraphRenameParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameParams {
    /// Current name.
    pub from: String,
    /// New unique name.
    pub to: String,
}

/// Adds an operation node.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio", name = "AudioGraphAddParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddParams {
    /// New unique node name.
    pub name: String,
    /// Operation (`lowpass`, `random.pick`).
    pub op: String,
    /// Arguments by input name: numbers, `@clip` references or `name`/`name.port` references.
    #[serde(default)]
    pub args: BTreeMap<String, Json>,
}

/// Removes a node or parameter and every reference to it.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio", name = "AudioGraphRemoveParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveParams {
    /// Name to remove.
    pub name: String,
}

/// Sets one input of a node, or a graph output, to a literal or a reference.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio", name = "AudioGraphSetInputParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetInputParams {
    /// Node name, or `output` for graph outputs.
    pub node: String,
    /// Input name (`cutoff`, `in0`) or output field (`out`, `left`, `right`, `done`).
    pub input: String,
    /// A number, an `@clip` reference or a `name`/`name.port` reference.
    pub value: Json,
}

/// Edits a parameter's default, range or documentation.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio", name = "AudioGraphParamParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParamParams {
    /// Parameter name.
    pub name: String,
    /// New default: a number, or an `@clip` reference for clip parameters.
    pub default: Option<Json>,
    /// New `[lo, hi]` range for value parameters.
    pub range: Option<[f32; 2]>,
    /// New documentation.
    pub doc: Option<String>,
}

fn err(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::GRAPH, message)
}

fn load(ctx: &EditCtx<'_, '_>) -> Result<(AudioGraph, Graph), Diagnostic> {
    let graph = Graph::from_document(ctx.read().doc())?;
    let audio = AudioGraph::from_graph(&graph).map_err(|d| {
        d.into_iter()
            .next()
            .unwrap_or_else(|| err("invalid audio graph"))
    })?;
    Ok((audio, graph))
}

fn save(
    ctx: &mut EditCtx<'_, '_>,
    audio: &AudioGraph,
    previous: Graph,
) -> Result<Json, Diagnostic> {
    let mut editor = previous.editor;
    editor.positions.retain(|id, _| {
        audio.params.contains_key(id)
            || audio.nodes.contains_key(id)
            || id == crate::projection::OUTPUT_ID
    });
    apply_snapshot(ctx, &audio.to_graph(editor)?)?;
    let diagnostics = check(audio).err().unwrap_or_default();
    Ok(json!({ "text": dsl::print(audio), "diagnostics": diagnostics }))
}

/// Parses a command value: number, `@clip`, or `name[.port]` reference.
fn value(audio: &AudioGraph, v: &Json) -> Result<Value, Diagnostic> {
    match v {
        Json::Number(n) => n
            .as_f64()
            .map(|f| Value::number(f as f32))
            .filter(|_| n.as_f64().is_some_and(f64::is_finite))
            .ok_or_else(|| err("numbers must be finite")),
        Json::String(s) if s.starts_with('@') => Ok(Value::Clip {
            reference: s.clone(),
        }),
        Json::String(s) => {
            let (name, port) = match s.split_once('.') {
                Some((n, p)) => (n, Some(p.to_owned())),
                None => (s.as_str(), None),
            };
            let (id, _) = audio
                .find(name)
                .ok_or_else(|| err(format!("unknown name `{name}`")))?;
            Ok(Value::Ref {
                node: id.to_owned(),
                port,
            })
        }
        _ => Err(err("expected a number, \"@clip\" or \"name[.port]\"")),
    }
}

fn node_id(audio: &AudioGraph, name: &str) -> Result<String, Diagnostic> {
    match audio.find(name) {
        Some((id, Item::Node(_))) => Ok(id.to_owned()),
        _ => Err(err(format!("no node named `{name}`"))),
    }
}

macro_rules! command {
    ($name:ident, $params:ty, $key:literal, $doc:literal, $example:expr, |$ctx:ident, $p:ident| $body:block) => {
        #[doc = $doc]
        pub struct $name;
        impl Command for $name {
            type Params = $params;
            type Output = Json;
            const NAME: &'static str = $key;
            const SUMMARY: &'static str = $doc;
            fn examples() -> Vec<Json> {
                vec![$example]
            }
            fn run($ctx: &mut EditCtx<'_, '_>, $p: Self::Params) -> Result<Json, Diagnostic> $body
        }
    };
}

command!(
    ApplyText,
    TextParams,
    "audiograph.text.apply",
    "Replace an audio graph with edited .audiograph text (names keep their IDs).",
    json!({"text": "graph Tone { param pitch = 440; let osc = sine(pitch); output out = osc; }"}),
    |ctx, p| {
        let (previous, graph) = load(ctx)?;
        let next = dsl::reconcile(&p.text, &previous)
            .map_err(|d| d.into_iter().next().unwrap_or_else(|| err("invalid text")))?;
        save(ctx, &next, graph)
    }
);

command!(
    Rename,
    RenameParams,
    "audiograph.rename",
    "Rename an audio graph parameter or node without changing its identity.",
    json!({"from": "osc", "to": "tone"}),
    |ctx, p| {
        let (mut audio, graph) = load(ctx)?;
        if audio.find(&p.to).is_some() {
            return Err(err(format!("`{}` already exists", p.to)));
        }
        let Some((id, _)) = audio.find(&p.from) else {
            return Err(err(format!("no parameter or node named `{}`", p.from)));
        };
        let id = id.to_owned();
        if let Some(param) = audio.params.get_mut(&id) {
            param.name = p.to;
        } else if let Some(node) = audio.nodes.get_mut(&id) {
            node.name = p.to;
        }
        save(ctx, &audio, graph)
    }
);

command!(
    AddNode,
    AddParams,
    "audiograph.node.add",
    "Add an operation node to an audio graph.",
    json!({"name": "filtered", "op": "lowpass", "args": {"in": "osc", "cutoff": 800}}),
    |ctx, p| {
        let (mut audio, graph) = load(ctx)?;
        if audio.find(&p.name).is_some() {
            return Err(err(format!("`{}` already exists", p.name)));
        }
        let op =
            Op::from_name(&p.op).ok_or_else(|| err(format!("unknown operation `{}`", p.op)))?;
        let mut args = BTreeMap::new();
        for (key, v) in &p.args {
            args.insert(key.clone(), value(&audio, v)?);
        }
        let order = audio.nodes.values().map(|n| n.order + 1).max().unwrap_or(0);
        let mut id = mint_id(&p.name);
        while audio.params.contains_key(&id) || audio.nodes.contains_key(&id) {
            id = mint_id(&format!("{id}+"));
        }
        audio.nodes.insert(
            id,
            Node {
                name: p.name,
                order,
                op,
                args,
                declared: None,
            },
        );
        save(ctx, &audio, graph)
    }
);

command!(
    RemoveNode,
    RemoveParams,
    "audiograph.node.remove",
    "Remove an audio graph node or parameter and every reference to it.",
    json!({"name": "filtered"}),
    |ctx, p| {
        let (mut audio, graph) = load(ctx)?;
        let Some((id, _)) = audio.find(&p.name) else {
            return Err(err(format!("no parameter or node named `{}`", p.name)));
        };
        let id = id.to_owned();
        audio.params.remove(&id);
        audio.nodes.remove(&id);
        let refers = |v: &Value| matches!(v, Value::Ref { node, .. } if *node == id);
        for node in audio.nodes.values_mut() {
            node.args.retain(|_, v| !refers(v));
        }
        audio.outputs.retain(|_, v| !refers(v));
        save(ctx, &audio, graph)
    }
);

command!(
    SetInput,
    SetInputParams,
    "audiograph.input.set",
    "Wire or set one audio graph input (or graph output) to a literal or another node.",
    json!({"node": "filtered", "input": "cutoff", "value": 1200}),
    |ctx, p| {
        let (mut audio, graph) = load(ctx)?;
        let v = value(&audio, &p.value)?;
        if p.node == "output" {
            let field = crate::ast::Output::parse(&p.input)
                .ok_or_else(|| err(format!("unknown output `{}`", p.input)))?;
            audio.outputs.insert(field, v);
        } else {
            let id = node_id(&audio, &p.node)?;
            let node = audio
                .nodes
                .get_mut(&id)
                .ok_or_else(|| err("node vanished"))?;
            let spec = node.op.spec();
            let variadic = p
                .input
                .strip_prefix("in")
                .and_then(|n| n.parse::<usize>().ok());
            if spec.input(&p.input).is_none()
                && !variadic.is_some_and(|n| spec.variadic.is_some_and(|v| n < v.max))
            {
                return Err(err(format!("`{}` has no input `{}`", spec.name, p.input)));
            }
            node.args.insert(p.input, v);
        }
        save(ctx, &audio, graph)
    }
);

command!(
    SetParam,
    ParamParams,
    "audiograph.param.set",
    "Edit an audio graph parameter's default, range or documentation.",
    json!({"name": "pitch", "default": 220, "range": [20, 2000]}),
    |ctx, p| {
        let (mut audio, graph) = load(ctx)?;
        let id = match audio.find(&p.name) {
            Some((id, Item::Param(_))) => id.to_owned(),
            _ => return Err(err(format!("no parameter named `{}`", p.name))),
        };
        let param = audio
            .params
            .get_mut(&id)
            .ok_or_else(|| err("parameter vanished"))?;
        match (&mut param.kind, &p.default) {
            (ParamKind::Value { default, .. }, Some(Json::Number(n))) => {
                *default = n.as_f64().unwrap_or_default() as f32;
            }
            (ParamKind::Clip { default }, Some(Json::String(s))) if s.starts_with('@') => {
                *default = s.clone();
            }
            (_, None) => {}
            _ => return Err(err("default does not match the parameter kind")),
        }
        if let Some(r) = p.range {
            match &mut param.kind {
                ParamKind::Value { range, .. } => *range = Some(r),
                _ => return Err(err("only value parameters have ranges")),
            }
        }
        if let Some(doc) = p.doc {
            param.doc = doc;
        }
        save(ctx, &audio, graph)
    }
);

/// Adds the audio-graph commands and their reflected parameter schemas.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<ApplyText>(db)
        .command::<Rename>(db)
        .command::<AddNode>(db)
        .command::<RemoveNode>(db)
        .command::<SetInput>(db)
        .command::<SetParam>(db);
}
