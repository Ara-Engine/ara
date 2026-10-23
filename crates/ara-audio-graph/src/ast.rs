//! The one audio-graph program. Text (`dsl`) and graph (`projection`) are views of it.
//! Identity is the node ID; names are unique and are what authors and agents write.

use crate::library::Op;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Signal type of a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Type {
    /// Audio-rate numbers (signals, parameters, constants).
    Audio,
    /// Sample-accurate event pulses.
    Trigger,
    /// A resident clip reference.
    Clip,
}

impl Type {
    /// DSL and graph port type name.
    pub fn name(self) -> &'static str {
        match self {
            Type::Audio => "audio",
            Type::Trigger => "trigger",
            Type::Clip => "clip",
        }
    }
    /// Parses a type name.
    pub fn parse(name: &str) -> Option<Type> {
        [Type::Audio, Type::Trigger, Type::Clip]
            .into_iter()
            .find(|t| t.name() == name)
    }
}

/// An argument or output value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Value {
    /// A numeric literal (an audio-rate constant).
    Number {
        /// The value.
        value: f32,
    },
    /// A clip asset reference, `@path` or `@path#id`.
    Clip {
        /// The reference as written.
        reference: String,
    },
    /// Another node's or parameter's output.
    Ref {
        /// Stable node ID.
        node: String,
        /// Output port; None means the first output.
        port: Option<String>,
    },
}

impl Value {
    /// A numeric literal.
    pub fn number(value: f32) -> Self {
        Value::Number { value }
    }
}

/// What a declared parameter carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ParamKind {
    /// A gameplay-set number with an optional allowed range.
    Value {
        /// Initial value.
        default: f32,
        /// Inclusive `[lo, hi]`; values set at runtime are clamped.
        range: Option<[f32; 2]>,
    },
    /// A gameplay-fired event.
    Trigger,
    /// A clip, overridable per instance.
    Clip {
        /// Default clip reference.
        default: String,
    },
}

/// A declared parameter, trigger or clip input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    /// Unique name.
    pub name: String,
    /// Declaration order.
    pub order: u32,
    /// Value, trigger or clip.
    pub kind: ParamKind,
    /// Tooltip and agent documentation.
    #[serde(default)]
    pub doc: String,
}

impl Param {
    /// The parameter's output type.
    pub fn ty(&self) -> Type {
        match self.kind {
            ParamKind::Value { .. } => Type::Audio,
            ParamKind::Trigger => Type::Trigger,
            ParamKind::Clip { .. } => Type::Clip,
        }
    }
}

/// One operation instance (`let name = op(args);`).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Unique name.
    pub name: String,
    /// Author order, the tie-break when printing in dependency order.
    pub order: u32,
    /// Operation.
    pub op: Op,
    /// Arguments keyed by port (`cutoff`, or `in0`.. for variadic items).
    pub args: BTreeMap<String, Value>,
    /// Optional declared type of the first output (`let x: audio = ...`).
    pub declared: Option<Type>,
}

/// Graph output fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Output {
    /// Mono output (spatialized voices).
    Out,
    /// Stereo left (non-spatial voices).
    Left,
    /// Stereo right.
    Right,
    /// Trigger that ends the voice.
    Done,
}

impl Output {
    /// Every field in printing order.
    pub const ALL: [Output; 4] = [Output::Out, Output::Left, Output::Right, Output::Done];
    /// DSL name.
    pub fn name(self) -> &'static str {
        match self {
            Output::Out => "out",
            Output::Left => "left",
            Output::Right => "right",
            Output::Done => "done",
        }
    }
    /// Parses a field name.
    pub fn parse(name: &str) -> Option<Output> {
        Output::ALL.into_iter().find(|o| o.name() == name)
    }
    /// Required type.
    pub fn ty(self) -> Type {
        match self {
            Output::Done => Type::Trigger,
            _ => Type::Audio,
        }
    }
}

/// A procedural audio graph.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioGraph {
    /// Graph asset ID (`gr_...`), supplied by the asset index, never written in text.
    pub id: String,
    /// Display name (`graph Footsteps { ... }`).
    pub name: String,
    /// Parameters keyed by node ID.
    pub params: BTreeMap<String, Param>,
    /// Operations keyed by node ID.
    pub nodes: BTreeMap<String, Node>,
    /// Output values.
    pub outputs: BTreeMap<Output, Value>,
}

/// What a name refers to.
#[derive(Debug, Clone, Copy)]
pub enum Item<'a> {
    /// A parameter.
    Param(&'a Param),
    /// An operation.
    Node(&'a Node),
}

impl AudioGraph {
    /// An empty graph.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            params: BTreeMap::new(),
            nodes: BTreeMap::new(),
            outputs: BTreeMap::new(),
        }
    }
    /// The ID and item named `name`.
    pub fn find(&self, name: &str) -> Option<(&str, Item<'_>)> {
        self.params
            .iter()
            .find(|(_, p)| p.name == name)
            .map(|(id, p)| (id.as_str(), Item::Param(p)))
            .or_else(|| {
                self.nodes
                    .iter()
                    .find(|(_, n)| n.name == name)
                    .map(|(id, n)| (id.as_str(), Item::Node(n)))
            })
    }
    /// The name of a parameter or node ID.
    pub fn name_of(&self, id: &str) -> Option<&str> {
        self.params
            .get(id)
            .map(|p| p.name.as_str())
            .or_else(|| self.nodes.get(id).map(|n| n.name.as_str()))
    }
}

/// Deterministic node ID for a new name: `n_` + 16 base32 characters of BLAKE3(name).
/// Existing graphs keep their IDs through renames (see `dsl::reconcile`).
pub fn mint_id(name: &str) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let hash = blake3::hash(name.as_bytes());
    let body: String = hash.as_bytes()[..16]
        .iter()
        .map(|b| ALPHABET[usize::from(b & 31)] as char)
        .collect();
    format!("n_{body}")
}
