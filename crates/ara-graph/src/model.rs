use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use ara_doc::{AssetId, Document, RecordId};
use ara_schema::{AssetRef, DocKindDesc, Schema, SchemaDb};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::codes;

/// Direction relative to the owning node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "GraphDirection", module = "graph")]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// Receives at most one connection.
    #[default]
    Input,
    /// Can feed any number of inputs.
    Output,
}

/// A stable named, nominally typed socket. Domains interpret `ty`.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "GraphPort", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Port {
    /// Domain type name, including semantic distinctions such as sRGB.
    pub ty: String,
    /// Input or output socket.
    #[serde(default)]
    pub direction: Direction,
    /// Tooltip and agent-facing documentation.
    pub doc: String,
    /// Whether compilation requires an incoming edge or a parameter of this name.
    #[serde(default)]
    pub required: bool,
}

/// A versioned node; opaque payloads survive unavailable plugins unchanged.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "n"), name = "GraphNode", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    /// Registry key, such as `material.add`.
    pub kind: String,
    /// Exact registry version used by this node.
    #[ara(default = 1)]
    #[serde(default = "one")]
    pub version: u32,
    /// Ports keyed by persistent socket identity, not display order.
    #[serde(default)]
    pub ports: BTreeMap<String, Port>,
    /// Literal input defaults and authorable parameters.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    /// Domain-owned data, preserved for unknown node kinds.
    #[serde(default)]
    pub payload: Value,
    /// Optional full `@path#id` subgraph reference.
    #[serde(default)]
    pub subgraph: Option<AssetRef>,
}

fn one() -> u32 {
    1
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.version == other.version
            && self.ports == other.ports
            && self.subgraph == other.subgraph
            && ara_json::json_eq(&self.payload, &other.payload)
            && self.params.len() == other.params.len()
            && self
                .params
                .iter()
                .all(|(k, v)| other.params.get(k).is_some_and(|w| ara_json::json_eq(v, w)))
    }
}

/// An addressed socket; port identity is scoped to the node.
#[derive(Debug, Clone, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "GraphEndpoint", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    /// Stable node record ID.
    pub node: String,
    /// Persistent socket key.
    pub port: String,
}

/// A directed connection with its own stable document record ID.
#[derive(Debug, Clone, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "w"), name = "GraphEdge", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edge {
    /// Output socket.
    pub from: Endpoint,
    /// Input socket.
    pub to: Endpoint,
}

/// Domain metadata, kept apart from editor state.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "g"), name = "GraphHeader", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    /// Adapter identity (`material`, `animation`, ...).
    pub domain: String,
    /// Domain-specific semantic metadata.
    #[serde(default)]
    pub payload: Value,
}

impl PartialEq for Header {
    fn eq(&self, other: &Self) -> bool {
        self.domain == other.domain && ara_json::json_eq(&self.payload, &other.payload)
    }
}

/// A graph comment, stored only in the editor section.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "GraphComment", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Comment {
    /// Human-readable annotation.
    pub text: String,
    /// Canvas position.
    pub position: [f32; 2],
}

/// A visual group; membership never changes execution.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "GraphGroup", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Group {
    /// Canvas title.
    pub label: String,
    /// Stable node IDs in the group.
    #[serde(default)]
    pub nodes: Vec<String>,
}

/// Nonsemantic editor state, excluded entirely from `semantic_hash`.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "l"), name = "GraphEditor", module = "graph")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Editor {
    /// Node coordinates keyed by stable semantic ID.
    #[serde(default)]
    pub positions: BTreeMap<String, [f32; 2]>,
    /// Comments keyed by stable comment ID.
    #[serde(default)]
    pub comments: BTreeMap<String, Comment>,
    /// Groups keyed by stable group ID.
    #[serde(default)]
    pub groups: BTreeMap<String, Group>,
    /// Edge routing points keyed by stable edge ID; not evaluation nodes.
    #[serde(default)]
    pub reroutes: BTreeMap<String, Vec<[f32; 2]>>,
}

/// A loaded graph projection. Construct/read freely; persist edits via commands.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    /// Project-unique graph asset ID (`gr_...`).
    pub id: AssetId,
    /// Domain identity and semantic metadata.
    pub header: Header,
    /// Nodes sorted by stable record ID.
    pub nodes: BTreeMap<String, Node>,
    /// Edges sorted by stable record ID.
    pub edges: BTreeMap<String, Edge>,
    /// Canvas data, never part of a shader cache key.
    pub editor: Editor,
}

/// Registers graph records and the canonical `graph` document kind.
pub fn register(db: &mut SchemaDb) -> Result<(), Diagnostic> {
    db.register::<Node>()
        .register::<Edge>()
        .register::<Header>()
        .register::<Editor>();
    db.add_kind(DocKindDesc {
        name: "graph".into(),
        version: 1,
        doc: "A domain-typed graph with stable node, edge and socket identities.".into(),
        id_prefix: "gr".into(),
        extension: "graph".into(),
        records: vec![
            "GraphNode".into(),
            "GraphEdge".into(),
            "GraphHeader".into(),
            "GraphEditor".into(),
        ],
        single_root: false,
    })
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Value, Diagnostic> {
    ara_json::to_value(value).map_err(|e| Diagnostic::error(codes::STRUCTURE, e.to_string()))
}

pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> Result<T, Diagnostic> {
    serde_json::from_value(value).map_err(|e| Diagnostic::error(codes::STRUCTURE, e.to_string()))
}

impl Graph {
    /// Creates an empty graph for a domain adapter.
    pub fn new(id: AssetId, domain: impl Into<String>) -> Self {
        Self {
            id,
            header: Header {
                domain: domain.into(),
                payload: Value::Null,
            },
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            editor: Editor::default(),
        }
    }

    /// Encodes an initial/imported snapshot into kernel document records.
    /// Subsequent authoring must use ara-cmd or ara-ops, never replace the snapshot.
    pub fn to_document(&self) -> Result<Document, Diagnostic> {
        let mut doc = Document::new("graph", self.id.clone());
        let mut put = |id: &str, value: Value| -> Result<(), Diagnostic> {
            let id = RecordId::parse(id).map_err(|e| Diagnostic::error(codes::STRUCTURE, e))?;
            let Value::Object(record) = value else {
                return Err(Diagnostic::error(
                    codes::STRUCTURE,
                    "record must be an object",
                ));
            };
            doc.insert_unchecked(id, record);
            Ok(())
        };
        put("g_meta", encode(&self.header)?)?;
        put("l_editor", encode(&self.editor)?)?;
        for (id, node) in &self.nodes {
            put(id, encode(node)?)?;
        }
        for (id, edge) in &self.edges {
            put(id, encode(edge)?)?;
        }
        Ok(doc)
    }

    /// Decodes records without discarding opaque node payloads.
    pub fn from_document(doc: &Document) -> Result<Self, Diagnostic> {
        if doc.kind != "graph" {
            return Err(Diagnostic::error(
                codes::STRUCTURE,
                "expected graph document",
            ));
        }
        let header = doc
            .record("g_meta")
            .ok_or_else(|| Diagnostic::error(codes::STRUCTURE, "missing g_meta record"))?;
        let mut graph = Self::new(doc.id.clone(), "");
        graph.header = decode(Value::Object(header.clone()))?;
        for (id, record) in doc.records() {
            let value = Value::Object(record.clone());
            match id.prefix() {
                "n" => {
                    graph.nodes.insert(id.to_string(), decode(value)?);
                }
                "w" => {
                    graph.edges.insert(id.to_string(), decode(value)?);
                }
                "l" if id.as_str() == "l_editor" => graph.editor = decode(value)?,
                "g" if id.as_str() == "g_meta" => (),
                _ => {
                    return Err(Diagnostic::error(
                        codes::STRUCTURE,
                        format!("unexpected graph record `{id}`"),
                    ));
                }
            }
        }
        Ok(graph)
    }

    /// Writes strict canonical JSON through the Schema DB and ara-json.
    pub fn write(&self, db: &SchemaDb) -> Result<String, Diagnostic> {
        Ok(ara_doc::write(&self.to_document()?, db))
    }

    /// Reads a canonical kernel graph document; unknown node kinds are retained.
    pub fn read(text: &str, db: &SchemaDb) -> Result<Self, Vec<Diagnostic>> {
        let result = ara_doc::read(text, "", db, None);
        if ara_diag::has_errors(&result.diags) {
            return Err(result.diags);
        }
        let doc = result.doc.ok_or(result.diags)?;
        Self::from_document(&doc).map_err(|d| vec![d])
    }

    /// BLAKE3 identity of canonical semantic records, independent of all layout.
    pub fn semantic_hash(&self, db: &SchemaDb) -> Result<blake3::Hash, Diagnostic> {
        let mut doc = self.to_document()?;
        // Rebuild an import snapshot excluding the editor record; no live mutation.
        let mut semantic = Document::new("graph", doc.id.clone());
        for (id, record) in doc.records() {
            if id.prefix() != "l" {
                semantic.insert_unchecked(id.clone(), record.clone());
            }
        }
        doc = semantic;
        Ok(blake3::hash(ara_doc::write(&doc, db).as_bytes()))
    }
}
