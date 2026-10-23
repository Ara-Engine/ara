use std::collections::{BTreeMap, BTreeSet};

use ara_diag::Diagnostic;
use ara_schema::grammar::{parse_ref, split_id};

use crate::{Direction, Graph, Node, Port, codes};

/// Searchable versioned node documentation; instances retain their own port IDs.
#[derive(Debug, Clone)]
pub struct NodeKind {
    /// Stable namespaced registry key.
    pub name: String,
    /// Exact node schema version.
    pub version: u32,
    /// Required tooltip/agent documentation.
    pub doc: String,
    /// Fixed socket definitions (empty for adapter-validated polymorphic nodes).
    pub ports: BTreeMap<String, Port>,
    /// Whether the domain adapter validates instantiated sockets.
    pub dynamic_ports: bool,
}

/// Node metadata indexed by (kind, version); registration never silently replaces.
#[derive(Debug, Default)]
pub struct NodeRegistry {
    kinds: BTreeMap<(String, u32), NodeKind>,
}

impl NodeRegistry {
    /// Registers documented metadata; duplicate identities are rejected.
    pub fn add(&mut self, kind: NodeKind) -> Result<(), Diagnostic> {
        let key = (kind.name.clone(), kind.version);
        if kind.name.is_empty()
            || kind.version == 0
            || kind.doc.trim().is_empty()
            || kind
                .ports
                .values()
                .any(|p| p.ty.is_empty() || p.doc.trim().is_empty())
            || self.kinds.contains_key(&key)
        {
            return Err(Diagnostic::error(
                codes::STRUCTURE,
                "node registry entry needs unique kind/version, types and docs",
            ));
        }
        self.kinds.insert(key, kind);
        Ok(())
    }

    /// Resolves an exact version; no implicit node upgrades.
    pub fn get(&self, name: &str, version: u32) -> Option<&NodeKind> {
        self.kinds.get(&(name.to_owned(), version))
    }

    /// Searches kind names and docs, in stable kind/version order.
    pub fn search<'a>(&'a self, query: &'a str) -> impl Iterator<Item = &'a NodeKind> {
        let query = query.to_lowercase();
        self.kinds.values().filter(move |k| {
            k.name.to_lowercase().contains(&query) || k.doc.to_lowercase().contains(&query)
        })
    }
}

/// Domain policy/compilation boundary. Animation can allow cycles; materials cannot.
pub trait DomainAdapter {
    /// Compiled domain-owned result.
    type Artifact;
    /// Header domain identity.
    fn name(&self) -> &str;
    /// Whether a connection can implicitly convert between nominal types.
    fn compatible(&self, output: &str, input: &str) -> bool {
        output == input
    }
    /// Whether feedback edges are legal.
    fn allows_cycles(&self) -> bool {
        false
    }
    /// Checks instantiated nodes, stage restrictions, defaults and domain rules.
    fn check_node(&self, _id: &str, _node: &Node) -> Vec<Diagnostic> {
        Vec::new()
    }
    /// Compiles a validated graph; failures retain last-good artifacts at the host.
    fn compile(&self, graph: &Graph) -> Result<Self::Artifact, Vec<Diagnostic>>;
}

impl Graph {
    /// Checks storage, registry metadata, wiring and domain-specific invariants.
    pub fn validate<D: DomainAdapter>(
        &self,
        registry: &NodeRegistry,
        domain: &D,
    ) -> Vec<Diagnostic> {
        let mut diags = self.check_wiring(|a, b| domain.compatible(a, b), domain.allows_cycles());
        if self.header.domain != domain.name() {
            diags.push(Diagnostic::error(
                codes::STRUCTURE,
                "graph header does not match the domain adapter",
            ));
        }
        for (id, node) in &self.nodes {
            match registry.get(&node.kind, node.version) {
                None => diags.push(
                    Diagnostic::error(
                        codes::UNKNOWN_NODE,
                        format!(
                            "node `{}` version {} is unavailable; install its plugin or replace it",
                            node.kind, node.version
                        ),
                    )
                    .record(id),
                ),
                Some(kind) if !kind.dynamic_ports && kind.ports != node.ports => diags.push(
                    Diagnostic::error(
                        codes::PORT,
                        "node sockets differ from its registered version",
                    )
                    .record(id),
                ),
                _ => (),
            }
            diags.extend(domain.check_node(id, node));
        }
        diags
    }

    /// Validates endpoint direction, single-input occupancy, nominal types and cycles.
    /// Commands use this independently of plugin availability to preserve unknown nodes.
    pub fn check_wiring(
        &self,
        compatible: impl Fn(&str, &str) -> bool,
        allows_cycles: bool,
    ) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let mut occupied = BTreeSet::new();
        for (id, node) in &self.nodes {
            if split_id(id).is_none_or(|(p, _)| p != "n") || node.version == 0 {
                diags.push(
                    Diagnostic::error(codes::STRUCTURE, "invalid node ID or version").record(id),
                );
            }
            for (port, desc) in &node.ports {
                if port.is_empty() || desc.ty.is_empty() || desc.doc.trim().is_empty() {
                    diags.push(
                        Diagnostic::error(codes::PORT, "socket needs stable ID, type and docs")
                            .record(id),
                    );
                }
                if desc.direction == Direction::Input
                    && desc.required
                    && !node.params.contains_key(port)
                    && !self
                        .edges
                        .values()
                        .any(|e| e.to.node == *id && e.to.port == *port)
                {
                    diags.push(
                        Diagnostic::error(
                            codes::PORT,
                            format!("required input `{port}` is unconnected"),
                        )
                        .record(id),
                    );
                }
            }
            if let Some(reference) = &node.subgraph
                && parse_ref(&reference.0).map_or(true, |r| r.path.is_none() || r.id.is_none())
            {
                diags.push(
                    Diagnostic::error(codes::STRUCTURE, "subgraph reference must be @path#id")
                        .record(id),
                );
            }
        }
        for (id, edge) in &self.edges {
            let from = self
                .nodes
                .get(&edge.from.node)
                .and_then(|n| n.ports.get(&edge.from.port));
            let to = self
                .nodes
                .get(&edge.to.node)
                .and_then(|n| n.ports.get(&edge.to.port));
            if split_id(id).is_none_or(|(p, _)| p != "w")
                || !matches!((from,to), (Some(a), Some(b)) if a.direction == Direction::Output
                    && b.direction == Direction::Input && compatible(&a.ty, &b.ty))
            {
                diags.push(
                    Diagnostic::error(
                        codes::PORT,
                        "edge needs existing, compatible output and input sockets",
                    )
                    .record(id),
                );
            }
            if !occupied.insert((&edge.to.node, &edge.to.port)) {
                diags.push(
                    Diagnostic::error(codes::PORT, "input already has a connection").record(id),
                );
            }
        }
        if !allows_cycles && self.topological_order().is_err() {
            diags.push(Diagnostic::error(
                codes::CYCLE,
                "domain requires an acyclic graph",
            ));
        }
        diags
    }

    /// Deterministic dependency order, including disconnected nodes; detects cycles.
    pub fn topological_order(&self) -> Result<Vec<&str>, Diagnostic> {
        let mut degree: BTreeMap<&str, usize> =
            self.nodes.keys().map(|id| (id.as_str(), 0)).collect();
        let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in self.edges.values() {
            if self.nodes.contains_key(&edge.from.node)
                && let Some(d) = degree.get_mut(edge.to.node.as_str())
            {
                *d += 1;
                successors
                    .entry(&edge.from.node)
                    .or_default()
                    .push(&edge.to.node);
            }
        }
        let mut ready: BTreeSet<&str> = degree
            .iter()
            .filter_map(|(&id, &d)| (d == 0).then_some(id))
            .collect();
        let mut result = Vec::with_capacity(degree.len());
        while let Some(id) = ready.pop_first() {
            result.push(id);
            for next in successors.get(id).into_iter().flatten() {
                if let Some(d) = degree.get_mut(next) {
                    *d -= 1;
                    if *d == 0 {
                        ready.insert(next);
                    }
                }
            }
        }
        if result.len() == degree.len() {
            Ok(result)
        } else {
            Err(Diagnostic::error(codes::CYCLE, "graph contains a cycle"))
        }
    }
}
