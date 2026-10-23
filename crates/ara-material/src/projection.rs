use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use ara_doc::AssetId;
use ara_graph::{Direction, DomainAdapter, Edge, Editor, Endpoint, Graph, Node, Port};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Binding, CustomFunction, Domain, Expr, Material, Parameter, Type, check, codes};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Metadata {
    name: String,
    domain: Domain,
    variant_budget: u32,
    functions: BTreeMap<String, CustomFunction>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeData {
    name: String,
    order: u32,
    ty: Type,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    function: Option<String>,
}

fn value<T: Serialize>(v: &T) -> Result<Value, Diagnostic> {
    ara_json::to_value(v).map_err(|e| Diagnostic::error(codes::SYNTAX, e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, Diagnostic> {
    serde_json::from_value(v).map_err(|e| Diagnostic::error(codes::SYNTAX, e.to_string()))
}
fn port(ty: Type, direction: Direction, required: bool, doc: &str) -> Port {
    Port {
        ty: ty.name().into(),
        direction,
        required,
        doc: doc.into(),
    }
}

/// Stable material edge identity, derived from destination node/socket only.
/// Rewiring an input therefore preserves reroute/layout identity.
pub fn edge_id(node: &str, port: &str) -> String {
    let hash = blake3::hash(format!("{node}/{port}").as_bytes()).to_hex();
    format!("w_{}", &hash[..20])
}

impl Material {
    /// Reads only identity metadata for text reconciliation, including draft graphs
    /// and unavailable node kinds. This context must never be compiled or persisted.
    pub(crate) fn name_context(g: &Graph) -> Result<Self, Diagnostic> {
        if g.header.domain != "material" {
            return Err(Diagnostic::error(codes::DOMAIN, "expected material graph"));
        }
        #[derive(Deserialize)]
        struct Identity {
            name: String,
            ty: Type,
        }
        let metadata: Metadata = decode(g.header.payload.clone())?;
        let mut context = Self {
            id: g.id.to_string(),
            name: metadata.name,
            domain: metadata.domain,
            variant_budget: metadata.variant_budget,
            functions: metadata.functions,
            parameters: BTreeMap::new(),
            bindings: BTreeMap::new(),
            outputs: BTreeMap::new(),
        };
        for (id, node) in &g.nodes {
            if id == "n_output" {
                continue;
            }
            let identity: Identity = decode(node.payload.clone()).map_err(|d| d.record(id))?;
            if node.kind == "material.parameter" {
                context.parameters.insert(
                    id.clone(),
                    Parameter {
                        name: identity.name,
                        ty: identity.ty,
                        order: context.parameters.len() as u32,
                        default: crate::ParameterValue::zero(identity.ty),
                        is_static: false,
                    },
                );
            } else {
                context.bindings.insert(
                    id.clone(),
                    Binding {
                        name: identity.name,
                        ty: identity.ty,
                        order: context.bindings.len() as u32,
                        expr: Expr::Literal {
                            values: vec![0.0; identity.ty.lanes()],
                        },
                    },
                );
            }
        }
        Ok(context)
    }

    /// Projects this AST into typed graph records, preserving supplied editor state.
    /// Calls have one output and ordered `in0`, `in1`, ... sockets; no second program.
    pub fn to_graph(&self, editor: Editor) -> Result<Graph, Diagnostic> {
        if self.node_type("n_output").is_some()
            || self
                .parameters
                .keys()
                .any(|id| self.bindings.contains_key(id))
        {
            return Err(Diagnostic::error(
                codes::SYNTAX,
                "material nodes overlap each other or the reserved output identity",
            ));
        }
        let id = AssetId::parse(&self.id).map_err(|e| Diagnostic::error(codes::SYNTAX, e))?;
        let mut g = Graph::new(id, "material");
        g.editor = editor;
        g.header.payload = value(&Metadata {
            name: self.name.clone(),
            domain: self.domain,
            variant_budget: self.variant_budget,
            functions: self.functions.clone(),
        })?;
        let make = |kind: String,
                    name: &str,
                    order: u32,
                    ty: Type,
                    function: Option<String>,
                    params: BTreeMap<String, Value>|
         -> Result<Node, Diagnostic> {
            Ok(Node {
                kind,
                version: 1,
                ports: BTreeMap::from([(
                    "out".into(),
                    port(ty, Direction::Output, false, "Typed expression result."),
                )]),
                params,
                payload: value(&NodeData {
                    name: name.into(),
                    order,
                    ty,
                    function,
                })?,
                subgraph: None,
            })
        };
        for (id, p) in &self.parameters {
            g.nodes.insert(
                id.clone(),
                make(
                    "material.parameter".into(),
                    &p.name,
                    p.order,
                    p.ty,
                    None,
                    BTreeMap::from([
                        ("value".into(), value(&p.default)?),
                        ("isStatic".into(), value(&p.is_static)?),
                    ]),
                )?,
            );
        }
        for (id, b) in &self.bindings {
            let (kind, function, params) = match &b.expr {
                Expr::Literal { values } => (
                    "material.literal".into(),
                    None,
                    BTreeMap::from([("value".into(), value(values)?)]),
                ),
                Expr::Call { op, .. } => match op.strip_prefix("custom.") {
                    Some(f) => ("material.custom".into(), Some(f.into()), BTreeMap::new()),
                    None => (format!("material.{op}"), None, BTreeMap::new()),
                },
            };
            let mut node = make(kind, &b.name, b.order, b.ty, function, params)?;
            if let Expr::Call { args, .. } = &b.expr {
                for (i, arg) in args.iter().enumerate() {
                    let ty = self.node_type(arg).ok_or_else(|| {
                        Diagnostic::error(codes::REFERENCE, format!("missing `{arg}`"))
                    })?;
                    let key = format!("in{i}");
                    node.ports.insert(
                        key.clone(),
                        port(ty, Direction::Input, true, "Ordered function argument."),
                    );
                    g.edges.insert(
                        edge_id(id, &key),
                        Edge {
                            from: Endpoint {
                                node: arg.clone(),
                                port: "out".into(),
                            },
                            to: Endpoint {
                                node: id.clone(),
                                port: key,
                            },
                        },
                    );
                }
            }
            g.nodes.insert(id.clone(), node);
        }
        let mut output = Node {
            kind: "material.output".into(),
            version: 1,
            ports: BTreeMap::new(),
            params: BTreeMap::new(),
            payload: Value::Null,
            subgraph: None,
        };
        for (field, ty) in self.domain.outputs() {
            output.ports.insert(
                field.into(),
                port(ty, Direction::Input, false, "Material domain output."),
            );
        }
        g.nodes.insert("n_output".into(), output);
        for (field, id) in &self.outputs {
            g.edges.insert(
                edge_id("n_output", field),
                Edge {
                    from: Endpoint {
                        node: id.clone(),
                        port: "out".into(),
                    },
                    to: Endpoint {
                        node: "n_output".into(),
                        port: field.clone(),
                    },
                },
            );
        }
        Ok(g)
    }

    /// Reconstructs the same AST from graph sockets. Rejects opaque/unknown nodes
    /// with diagnostics; the source Graph continues to retain all their payloads.
    pub fn from_graph(g: &Graph) -> Result<Self, Vec<Diagnostic>> {
        Self::from_graph_inner(g).map_err(|d| vec![d])
    }

    fn from_graph_inner(g: &Graph) -> Result<Self, Diagnostic> {
        if g.header.domain != "material" {
            return Err(Diagnostic::error(codes::DOMAIN, "expected material graph"));
        }
        if let Some(d) = g.check_wiring(|_, _| true, false).into_iter().next() {
            return Err(d);
        }
        let meta: Metadata = decode(g.header.payload.clone())?;
        let mut m = Material {
            id: g.id.to_string(),
            name: meta.name,
            domain: meta.domain,
            variant_budget: meta.variant_budget,
            parameters: BTreeMap::new(),
            bindings: BTreeMap::new(),
            functions: meta.functions,
            outputs: BTreeMap::new(),
        };
        let mut incoming = BTreeMap::new();
        for (id, edge) in &g.edges {
            if *id != edge_id(&edge.to.node, &edge.to.port) || edge.from.port != "out" {
                return Err(Diagnostic::error(codes::SYNTAX,"material edge identity must derive from destination socket; use material.connect").record(id));
            }
            incoming.insert(
                (edge.to.node.as_str(), edge.to.port.as_str()),
                edge.from.node.clone(),
            );
        }
        for (id, node) in &g.nodes {
            if node.version != 1 || node.subgraph.is_some() {
                return Err(Diagnostic::error(
                    ara_graph::codes::UNKNOWN_NODE,
                    "unsupported material node version or subgraph; data retained",
                )
                .record(id));
            }
            if id == "n_output" {
                if node.kind != "material.output" {
                    return Err(Diagnostic::error(
                        codes::SYNTAX,
                        "n_output must be the domain output node",
                    ));
                }
                for ((target, field), source) in &incoming {
                    if *target == "n_output" {
                        m.outputs.insert((*field).into(), source.clone());
                    }
                }
                continue;
            }
            let data: NodeData = decode(node.payload.clone()).map_err(|d| d.record(id))?;
            if node.kind == "material.parameter" {
                m.parameters.insert(
                    id.clone(),
                    Parameter {
                        name: data.name,
                        order: data.order,
                        ty: data.ty,
                        default: decode(node.params.get("value").cloned().unwrap_or(json!([])))?,
                        is_static: decode(
                            node.params.get("isStatic").cloned().unwrap_or(json!(false)),
                        )?,
                    },
                );
            } else {
                let expr = if node.kind == "material.literal" {
                    Expr::Literal {
                        values: decode(node.params.get("value").cloned().unwrap_or(json!([])))?,
                    }
                } else {
                    let op = if node.kind == "material.custom" {
                        format!(
                            "custom.{}",
                            data.function.ok_or_else(|| Diagnostic::error(
                                codes::REFERENCE,
                                "custom node needs function name"
                            ))?
                        )
                    } else {
                        node.kind
                            .strip_prefix("material.")
                            .filter(|op| crate::library::get(op).is_some())
                            .ok_or_else(|| {
                                Diagnostic::error(
                                    ara_graph::codes::UNKNOWN_NODE,
                                    format!("unknown material node `{}`; data retained", node.kind),
                                )
                                .record(id)
                            })?
                            .to_owned()
                    };
                    let count = node
                        .ports
                        .values()
                        .filter(|p| p.direction == Direction::Input)
                        .count();
                    let mut args = Vec::new();
                    for i in 0..count {
                        let key = format!("in{i}");
                        args.push(
                            incoming
                                .get(&(id.as_str(), key.as_str()))
                                .ok_or_else(|| {
                                    Diagnostic::error(
                                        codes::REFERENCE,
                                        format!("missing argument `{key}`"),
                                    )
                                    .record(id)
                                })?
                                .clone(),
                        );
                    }
                    Expr::Call { op, args }
                };
                m.bindings.insert(
                    id.clone(),
                    Binding {
                        name: data.name,
                        order: data.order,
                        ty: data.ty,
                        expr,
                    },
                );
            }
        }
        let canonical = m.to_graph(g.editor.clone())?;
        if canonical.nodes != g.nodes || canonical.edges != g.edges {
            return Err(Diagnostic::error(
                codes::SYNTAX,
                "graph contains noncanonical material sockets or data",
            ));
        }
        Ok(m)
    }
}

/// Material policy for the shared graph framework; compile output is the typed AST.
pub struct MaterialAdapter;
impl DomainAdapter for MaterialAdapter {
    type Artifact = Material;
    fn name(&self) -> &str {
        "material"
    }
    fn compile(&self, g: &Graph) -> Result<Material, Vec<Diagnostic>> {
        let m = Material::from_graph(g)?;
        check(&m)?;
        Ok(m)
    }
}
