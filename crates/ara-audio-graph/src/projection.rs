//! The graph view of an `AudioGraph`, stored as an `ara-graph` document (domain `audio`).
//!
//! Each parameter and operation is a node (`audio.param`, `audio.lowpass`, ...) whose
//! payload carries its name and author order; literal arguments live in node `params`,
//! references are edges. Edge IDs derive from the destination socket, so rewiring an
//! input keeps its identity. Outputs are the reserved `n_output` node.

use crate::{
    ast::{AudioGraph, Node, Output, Param, ParamKind, Type, Value},
    check::{Checked, check},
    codes,
    library::{Op, TypeRule},
};
use ara_diag::Diagnostic;
use ara_doc::AssetId;
use ara_graph::{Direction, DomainAdapter, Edge, Editor, Endpoint, Graph, Node as GraphNode, Port};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

/// Reserved ID of the outputs node.
pub const OUTPUT_ID: &str = "n_output";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeData {
    name: String,
    order: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    doc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    declared: Option<Type>,
}

/// Stable edge identity from the destination node and socket.
pub fn edge_id(node: &str, port: &str) -> String {
    let hash = blake3::hash(format!("{node}/{port}").as_bytes()).to_hex();
    format!("w_{}", &hash[..20])
}

fn err(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::GRAPH, message)
}

fn encode<T: Serialize>(value: &T) -> Result<Json, Diagnostic> {
    ara_json::to_value(value).map_err(|e| err(e.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(value: Json) -> Result<T, Diagnostic> {
    serde_json::from_value(value).map_err(|e| err(e.to_string()))
}

fn port(ty: Type, direction: Direction, doc: &str) -> Port {
    Port {
        ty: ty.name().into(),
        direction,
        required: false,
        doc: doc.into(),
    }
}

/// JSON for an f32 (or f32 array) at its shortest round-trip precision.
fn tidy<T: Serialize>(value: &T) -> Json {
    ara_json::to_value(value).unwrap_or(Json::Null)
}

fn literal(value: &Value) -> Option<Json> {
    match value {
        Value::Number { value } => Some(tidy(value)),
        Value::Clip { reference } => Some(json!(reference)),
        Value::Ref { .. } => None,
    }
}

impl AudioGraph {
    /// Projects into graph records, keeping supplied canvas state.
    pub fn to_graph(&self, editor: Editor) -> Result<Graph, Diagnostic> {
        let id = AssetId::parse(&self.id).map_err(err)?;
        let types = check(self).map(|c| c.types).unwrap_or_default();
        let mut g = Graph::new(id, "audio");
        g.editor = editor;
        g.header.payload = encode(&Header {
            name: self.name.clone(),
        })?;
        let mut edges = BTreeMap::new();
        let mut connect = |to: &str, socket: &str, value: &Value| {
            if let Value::Ref { node, port } = value {
                let from_port = match port {
                    Some(p) => p.clone(),
                    None => self
                        .nodes
                        .get(node)
                        .map_or("out", |n| n.op.spec().outputs[0].name)
                        .to_owned(),
                };
                edges.insert(
                    edge_id(to, socket),
                    Edge {
                        from: Endpoint {
                            node: node.clone(),
                            port: from_port,
                        },
                        to: Endpoint {
                            node: to.to_owned(),
                            port: socket.to_owned(),
                        },
                    },
                );
            }
        };
        for (id, p) in &self.params {
            let (kind, params) = match &p.kind {
                ParamKind::Value { default, range } => {
                    let mut params = BTreeMap::from([("default".to_owned(), tidy(default))]);
                    if let Some(r) = range {
                        params.insert("range".into(), tidy(r));
                    }
                    ("audio.param", params)
                }
                ParamKind::Trigger => ("audio.trigger", BTreeMap::new()),
                ParamKind::Clip { default } => (
                    "audio.clip",
                    BTreeMap::from([("default".to_owned(), json!(default))]),
                ),
            };
            g.nodes.insert(
                id.clone(),
                GraphNode {
                    kind: kind.into(),
                    version: 1,
                    ports: BTreeMap::from([(
                        "out".to_owned(),
                        port(p.ty(), Direction::Output, "Value"),
                    )]),
                    params,
                    payload: encode(&NodeData {
                        name: p.name.clone(),
                        order: p.order,
                        doc: p.doc.clone(),
                        declared: None,
                    })?,
                    subgraph: None,
                },
            );
        }
        for (id, node) in &self.nodes {
            let spec = node.op.spec();
            let produced = types.get(id);
            let generic = spec
                .outputs
                .iter()
                .position(|o| o.ty == TypeRule::Generic)
                .and_then(|i| produced.and_then(|t| t.get(i).copied()))
                .unwrap_or(Type::Audio);
            let resolve = |rule: TypeRule| match rule {
                TypeRule::Is(t) => t,
                TypeRule::Generic => generic,
            };
            let mut ports = BTreeMap::new();
            for input in spec.inputs {
                ports.insert(
                    input.name.to_owned(),
                    port(resolve(input.ty), Direction::Input, input.doc),
                );
            }
            if let Some(v) = spec.variadic {
                for key in node
                    .args
                    .keys()
                    .filter(|k| k.starts_with("in") && spec.input(k).is_none())
                {
                    ports.insert(key.clone(), port(resolve(v.ty), Direction::Input, v.doc));
                }
            }
            for output in spec.outputs {
                ports.insert(
                    output.name.to_owned(),
                    port(resolve(output.ty), Direction::Output, output.doc),
                );
            }
            let mut params = BTreeMap::new();
            for (socket, value) in &node.args {
                match literal(value) {
                    Some(v) => {
                        params.insert(socket.clone(), v);
                    }
                    None => connect(id, socket, value),
                }
            }
            g.nodes.insert(
                id.clone(),
                GraphNode {
                    kind: node.op.kind(),
                    version: 1,
                    ports,
                    params,
                    payload: encode(&NodeData {
                        name: node.name.clone(),
                        order: node.order,
                        doc: String::new(),
                        declared: node.declared,
                    })?,
                    subgraph: None,
                },
            );
        }
        let mut ports = BTreeMap::new();
        let mut params = BTreeMap::new();
        for (field, value) in &self.outputs {
            ports.insert(
                field.name().to_owned(),
                port(field.ty(), Direction::Input, "Graph output"),
            );
            match literal(value) {
                Some(v) => {
                    params.insert(field.name().to_owned(), v);
                }
                None => connect(OUTPUT_ID, field.name(), value),
            }
        }
        g.nodes.insert(
            OUTPUT_ID.into(),
            GraphNode {
                kind: "audio.output".into(),
                version: 1,
                ports,
                params,
                payload: Json::Null,
                subgraph: None,
            },
        );
        g.edges = edges;
        Ok(g)
    }

    /// Rebuilds the program from graph records.
    pub fn from_graph(g: &Graph) -> Result<Self, Vec<Diagnostic>> {
        if g.header.domain != "audio" {
            return Err(vec![err("expected an `audio` graph")]);
        }
        let header: Header = decode(g.header.payload.clone()).map_err(|d| vec![d])?;
        let mut graph = AudioGraph::new(g.id.to_string(), header.name);
        let mut diags = Vec::new();
        let incoming = |node: &str, socket: &str| -> Option<Value> {
            g.edges
                .values()
                .find(|e| e.to.node == node && e.to.port == socket)
                .map(|e| {
                    let first = g
                        .nodes
                        .get(&e.from.node)
                        .and_then(|n| n.kind.strip_prefix("audio."))
                        .and_then(Op::from_name)
                        .map_or("out", |op| op.spec().outputs[0].name);
                    Value::Ref {
                        node: e.from.node.clone(),
                        port: (e.from.port != first).then(|| e.from.port.clone()),
                    }
                })
        };
        let from_literal = |v: &Json| -> Option<Value> {
            match v {
                Json::Number(n) => n.as_f64().map(|f| Value::number(f as f32)),
                Json::String(s) if s.starts_with('@') => Some(Value::Clip {
                    reference: s.clone(),
                }),
                _ => None,
            }
        };
        for (id, node) in &g.nodes {
            if id == OUTPUT_ID {
                for (socket, desc) in &node.ports {
                    let Some(field) =
                        Output::parse(socket).filter(|_| desc.direction == Direction::Input)
                    else {
                        diags.push(err(format!("unknown output `{socket}`")).record(id));
                        continue;
                    };
                    let value = incoming(id, socket)
                        .or_else(|| node.params.get(socket).and_then(from_literal));
                    if let Some(value) = value {
                        graph.outputs.insert(field, value);
                    }
                }
                continue;
            }
            let data: NodeData = match decode(node.payload.clone()) {
                Ok(d) => d,
                Err(d) => {
                    diags.push(d.record(id));
                    continue;
                }
            };
            let number = |key: &str| {
                node.params
                    .get(key)
                    .and_then(Json::as_f64)
                    .map(|f| f as f32)
            };
            let param = |kind| Param {
                name: data.name.clone(),
                order: data.order,
                kind,
                doc: data.doc.clone(),
            };
            match node.kind.as_str() {
                "audio.param" => {
                    let range = node
                        .params
                        .get("range")
                        .and_then(|r| serde_json::from_value::<[f32; 2]>(r.clone()).ok());
                    graph.params.insert(
                        id.clone(),
                        param(ParamKind::Value {
                            default: number("default").unwrap_or(0.0),
                            range,
                        }),
                    );
                }
                "audio.trigger" => {
                    graph.params.insert(id.clone(), param(ParamKind::Trigger));
                }
                "audio.clip" => {
                    let default = node
                        .params
                        .get("default")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    graph
                        .params
                        .insert(id.clone(), param(ParamKind::Clip { default }));
                }
                kind => {
                    let Some(op) = kind.strip_prefix("audio.").and_then(Op::from_name) else {
                        diags.push(err(format!("unknown audio node kind `{kind}`")).record(id));
                        continue;
                    };
                    let mut args = BTreeMap::new();
                    for socket in node
                        .ports
                        .iter()
                        .filter(|(_, p)| p.direction == Direction::Input)
                        .map(|(k, _)| k)
                    {
                        let value = incoming(id, socket)
                            .or_else(|| node.params.get(socket).and_then(from_literal));
                        if let Some(value) = value {
                            args.insert(socket.clone(), value);
                        }
                    }
                    graph.nodes.insert(
                        id.clone(),
                        Node {
                            name: data.name,
                            order: data.order,
                            op,
                            args,
                            declared: data.declared,
                        },
                    );
                }
            }
        }
        if diags.is_empty() {
            Ok(graph)
        } else {
            Err(diags)
        }
    }
}

/// `ara-graph` domain adapter for audio graphs: acyclic, nominal types, compiles to `Checked`.
pub struct AudioAdapter;

impl DomainAdapter for AudioAdapter {
    type Artifact = Checked;
    fn name(&self) -> &str {
        "audio"
    }
    fn check_node(&self, id: &str, node: &GraphNode) -> Vec<Diagnostic> {
        let known = id == OUTPUT_ID
            || matches!(
                node.kind.as_str(),
                "audio.param" | "audio.trigger" | "audio.clip" | "audio.output"
            )
            || node
                .kind
                .strip_prefix("audio.")
                .and_then(Op::from_name)
                .is_some();
        if known {
            Vec::new()
        } else {
            vec![err(format!("unknown audio node kind `{}`", node.kind)).record(id)]
        }
    }
    fn compile(&self, graph: &Graph) -> Result<Checked, Vec<Diagnostic>> {
        check(&AudioGraph::from_graph(graph)?)
    }
}
