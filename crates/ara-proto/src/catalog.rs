//! The method catalog: what `rpc.discover` returns and the MCP adapter builds its
//! tools from. Params schemas are JSON Schema 2020-12 generated from the Schema DB
//! (the params types' doc comments become descriptions).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Scope;

/// Behaviour hints, mirrored into MCP tool annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodHints {
    /// The method never changes documents or engine state.
    pub read_only: bool,
    /// The method can remove data.
    pub destructive: bool,
    /// Repeating the call changes nothing further.
    pub idempotent: bool,
}

/// Which MCP tool a method backs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBinding {
    /// Tool name (`ara_session`).
    pub tool: String,
    /// The tool's `action` value selecting this method, for tools that dispatch on
    /// one; `None` when the method is the tool's only one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

/// One Engine Protocol method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodInfo {
    /// Method name (`cmd.apply`).
    pub name: String,
    /// One line on what it does.
    pub summary: String,
    /// Scope the session needs; `None` for methods that need no session
    /// (`session.begin`, `rpc.discover`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// Behaviour hints.
    pub hints: MethodHints,
    /// The MCP tool this method backs, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolBinding>,
    /// JSON Schema 2020-12 of the params object.
    pub params: Value,
    /// The service that provides it (`core`, `play`).
    pub service: String,
}
