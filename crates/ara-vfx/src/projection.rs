use crate::{Effect, StackModule, Stacks, decode, encode, error};
use ara_diag::Diagnostic;
use ara_doc::AssetId;
use ara_graph::{Direction, DomainAdapter, Edge, Editor, Endpoint, Graph, Node, Port};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
struct Header {
    effect: Effect,
}
#[derive(Serialize, Deserialize)]
struct Item {
    emitter: usize,
    stage: crate::Stage,
    order: usize,
    name: String,
    bindings: BTreeMap<String, String>,
}
fn identity(asset: &str, emitter: &str, module: &str) -> String {
    format!(
        "n_{}",
        &blake3::hash(format!("{asset}/{emitter}/{module}").as_bytes()).to_hex()[..20]
    )
}
fn ports() -> BTreeMap<String, Port> {
    [("in", Direction::Input), ("out", Direction::Output)]
        .into_iter()
        .map(|(name, direction)| {
            (
                name.into(),
                Port {
                    ty: "vfx.stack".into(),
                    direction,
                    required: false,
                    doc: "Ordered stack execution dependency.".into(),
                },
            )
        })
        .collect()
}
impl Effect {
    /// Project into per-module graph records, with ordered execution edges.
    /// Only initial/imported snapshots may be installed directly in a Store.
    pub fn to_graph(&self, asset: AssetId, editor: Editor) -> Result<Graph, Diagnostic> {
        self.validate()?;
        let mut graph = Graph::new(asset, "vfx");
        let mut header = self.clone();
        for e in &mut header.emitters {
            e.stacks = Stacks::default();
        }
        graph.header.payload = encode(&Header { effect: header })?;
        graph.editor = editor;
        for (emitter, e) in self.emitters.iter().enumerate() {
            let mut previous = None::<String>;
            for stage in Stacks::STAGES {
                for (order, item) in e.stacks.get(stage).iter().enumerate() {
                    let id = identity(graph.id.as_str(), &e.name, &item.name);
                    graph.nodes.insert(
                        id.clone(),
                        Node {
                            kind: format!("vfx.{}", item.module.kind()),
                            version: 1,
                            ports: ports(),
                            params: BTreeMap::from([("module".into(), encode(&item.module)?)]),
                            payload: encode(&Item {
                                emitter,
                                stage,
                                order,
                                name: item.name.clone(),
                                bindings: item.bindings.clone(),
                            })?,
                            subgraph: None,
                        },
                    );
                    if let Some(from) = previous {
                        graph.edges.insert(
                            format!("w_{}", &id[2..]),
                            Edge {
                                from: Endpoint {
                                    node: from,
                                    port: "out".into(),
                                },
                                to: Endpoint {
                                    node: id.clone(),
                                    port: "in".into(),
                                },
                            },
                        );
                    }
                    previous = Some(id);
                }
            }
        }
        Ok(graph)
    }
    /// Decode and validate the authoritative stack graph, rejecting contradictory edges.
    pub fn from_graph(graph: &Graph) -> Result<Self, Diagnostic> {
        if graph.header.domain != "vfx" {
            return Err(error("expected vfx graph"));
        }
        let mut effect = decode::<Header>(graph.header.payload.clone())?.effect;
        if effect
            .emitters
            .iter()
            .any(|e| Stacks::STAGES.iter().any(|s| !e.stacks.get(*s).is_empty()))
        {
            return Err(error(
                "graph header cannot contain duplicate module storage",
            ));
        }
        let mut items = Vec::new();
        for (id, node) in &graph.nodes {
            let data: Item = decode(node.payload.clone())?;
            if node.version != 1
                || node.subgraph.is_some()
                || node.ports != ports()
                || node.params.len() != 1
            {
                return Err(error("unsupported VFX node version, ports or data").record(id));
            }
            let module = decode(
                node.params
                    .get("module")
                    .cloned()
                    .ok_or_else(|| error("missing typed module"))?,
            )?;
            items.push((data, module));
        }
        items.sort_by_key(|(d, _)| {
            (
                d.emitter,
                Stacks::STAGES.iter().position(|s| *s == d.stage),
                d.order,
            )
        });
        for (d, module) in items {
            let emitter = effect
                .emitters
                .get_mut(d.emitter)
                .ok_or_else(|| error("unknown emitter"))?;
            let stack = emitter.stacks.get_mut(d.stage);
            if d.order != stack.len() {
                return Err(error("stack positions must be contiguous and unique"));
            }
            stack.push(StackModule {
                name: d.name,
                module,
                bindings: d.bindings,
            });
        }
        effect.validate()?;
        let expected = effect.to_graph(graph.id.clone(), graph.editor.clone())?;
        if graph.nodes != expected.nodes || graph.edges != expected.edges {
            return Err(error(
                "stack nodes/edges contradict their identity or authored order",
            ));
        }
        Ok(effect)
    }
}
/// Domain adapter used before publishing checked module artifacts.
pub struct VfxAdapter;
impl DomainAdapter for VfxAdapter {
    type Artifact = Effect;
    fn name(&self) -> &str {
        "vfx"
    }
    fn compile(&self, graph: &Graph) -> Result<Effect, Vec<Diagnostic>> {
        Effect::from_graph(graph).map_err(|d| vec![d])
    }
}

/// Registry metadata for all built-in modules, derived from their documented type.
pub fn registry() -> Result<ara_graph::NodeRegistry, Diagnostic> {
    let mut registry = ara_graph::NodeRegistry::default();
    for module in crate::library::MODULES {
        registry.add(ara_graph::NodeKind {
            name: format!("vfx.{}", module.name),
            version: 1,
            doc: module.doc.into(),
            ports: ports(),
            dynamic_ports: false,
        })?;
    }
    Ok(registry)
}
