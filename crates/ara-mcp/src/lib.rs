//! The MCP adapter (A5, R03 §4.5): a thin, generated view of the Engine Protocol.
//!
//! - Twelve tools in a fixed order ([`tools::TOOLS`]); their input schemas are
//!   generated from the live method catalog (`rpc.discover`), so they follow the
//!   Schema DB and its doc comments. A tool lights up as soon as a service
//!   registers methods bound to it; until then calls return the host's `ARA1806`.
//! - Stateless per MCP 2026-07-28: no handshake state, the session handle is an
//!   ordinary tool argument. `server/discover` and per-request `_meta` versions are
//!   served; legacy clients get `initialize`/`ping` too (dual-era).
//! - Tool results carry the method result as `structuredContent` and its text
//!   (scene-to-text when the method returns `text`) as content. Engine errors are
//!   tool errors (`isError`) with the diagnostics, so the model can self-correct.
//!
//! The adapter holds no engine state: every call goes to an
//! [`Endpoint`](ara_proto::Endpoint) (the daemon client, or the host in-process).

pub mod tools;

use std::io::{self, BufRead, Write};

use ara_diag::{Diagnostic, FixAction, codes};
use ara_proto::{Endpoint, Incoming, MethodInfo, RpcError, jsonrpc, lines};
use serde_json::{Map, Value, json};

/// Protocol versions served, newest first. Older ones need `initialize`.
pub const SUPPORTED: [&str; 4] = ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"];
/// The stateless revision.
pub const MODERN: &str = "2026-07-28";
/// `UnsupportedProtocolVersionError`.
pub const UNSUPPORTED_VERSION: i64 = -32022;
/// `HeaderMismatch`.
pub const HEADER_MISMATCH: i64 = -32020;
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

const INSTRUCTIONS: &str = "Ara game engine tools. Start with ara_session {\"action\": \"begin\", \"client\": \"<your name>\"} and pass the returned `session` to every tool. Find commands and components with ara_schema {\"action\": \"search\"}; read scenes with ara_inspect and ara_query; edit only through ara_apply (atomic batches with a `why`, symbolic IDs via `as`). Conventions: Y-up, meters, degrees, IDs like e_8b21, references @path#id. Diagnostics carry stable ARA codes and fixes.";

fn server_info() -> Value {
    json!({ "name": "ara", "title": "Ara engine", "version": env!("CARGO_PKG_VERSION") })
}

/// Adds the fields every modern result carries.
fn complete(mut result: Value) -> Value {
    if let Some(o) = result.as_object_mut() {
        o.insert("resultType".into(), "complete".into());
        o.insert(
            "_meta".into(),
            json!({ "io.modelcontextprotocol/serverInfo": server_info() }),
        );
    }
    result
}

fn rpc_error(code: i64, message: impl Into<String>, data: Option<Value>) -> RpcError {
    RpcError {
        code,
        message: message.into(),
        data,
    }
}

/// The adapter. Caches the method catalog between calls.
#[derive(Default)]
pub struct Adapter {
    catalog: Option<Vec<MethodInfo>>,
}

impl Adapter {
    /// A new adapter.
    pub fn new() -> Self {
        Self::default()
    }

    fn refresh(&mut self, ep: &mut dyn Endpoint) -> Result<&[MethodInfo], RpcError> {
        let discovered = ep.call("rpc.discover", json!({}))?;
        let methods = serde_json::from_value(discovered["methods"].clone())
            .map_err(|e| rpc_error(jsonrpc::ENGINE_ERROR, format!("bad catalog: {e}"), None))?;
        Ok(self.catalog.insert(methods))
    }

    /// The tool definitions (always regenerated from a fresh catalog).
    pub fn tools(&mut self, ep: &mut dyn Endpoint) -> Result<Vec<Value>, RpcError> {
        let catalog = self.refresh(ep)?;
        Ok(tools::definitions(catalog))
    }

    /// Handles one MCP JSON-RPC message; returns the response (none for
    /// notifications).
    pub fn handle(&mut self, ep: &mut dyn Endpoint, msg: Value) -> Option<Value> {
        let (id, method, params) = match Incoming::from_value(msg) {
            Ok(Incoming::Request { id, method, params }) => (id, method, params),
            Ok(_) => return None,
            Err(error) => return Some(error),
        };
        if let Some(v) = params
            .get("_meta")
            .and_then(|m| m.get(VERSION_KEY))
            .and_then(Value::as_str)
            && !SUPPORTED.contains(&v)
        {
            let err = rpc_error(
                UNSUPPORTED_VERSION,
                "Unsupported protocol version",
                Some(json!({ "supported": SUPPORTED, "requested": v })),
            );
            return Some(jsonrpc::error_response(id, &err));
        }
        Some(jsonrpc::reply(id, self.dispatch(ep, &method, params)))
    }

    fn dispatch(
        &mut self,
        ep: &mut dyn Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        match method {
            "server/discover" => Ok(complete(json!({
                "supportedVersions": SUPPORTED,
                "capabilities": { "tools": { "listChanged": false } },
                "instructions": INSTRUCTIONS,
                "ttlMs": 60_000,
                "cacheScope": "private",
            }))),
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked
                    .filter(|v| SUPPORTED[1..].contains(v))
                    .unwrap_or(SUPPORTED[1]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": server_info(),
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(complete(json!({
                "tools": self.tools(ep)?,
                "ttlMs": 5_000,
                "cacheScope": "private",
            }))),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let args = params
                    .get("arguments")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                self.call_tool(ep, &name, args).map(complete)
            }
            other => Err(rpc_error(
                jsonrpc::METHOD_NOT_FOUND,
                format!("Method not found: {other}"),
                None,
            )),
        }
    }

    /// Calls a tool: routes it (by `action` for multi-method tools) to its method.
    /// Unknown tools are protocol errors; everything else is a tool result.
    pub fn call_tool(
        &mut self,
        ep: &mut dyn Endpoint,
        name: &str,
        mut args: Map<String, Value>,
    ) -> Result<Value, RpcError> {
        let Some(spec) = tools::TOOLS.iter().find(|t| t.name == name) else {
            return Err(rpc_error(
                jsonrpc::INVALID_PARAMS,
                format!("Unknown tool: {name}"),
                None,
            ));
        };
        if self
            .catalog
            .as_ref()
            .is_none_or(|c| tools::methods_of(c, name).is_empty())
        {
            self.refresh(ep)?;
        }
        let catalog = self.catalog.as_deref().unwrap_or_default();
        let methods = tools::methods_of(catalog, name);
        let method = match methods.as_slice() {
            [] => args
                .remove("action")
                .and_then(|a| a.as_str().map(|a| format!("{}.{a}", spec.reserved)))
                .unwrap_or_else(|| spec.reserved.to_owned()),
            [m] if m.tool.as_ref().is_some_and(|t| t.action.is_none()) => m.name.clone(),
            many => {
                let action = args.remove("action");
                let wanted = action.as_ref().and_then(Value::as_str);
                let found = many.iter().find(|m| {
                    m.tool.as_ref().and_then(|t| t.action.as_deref()) == wanted && wanted.is_some()
                });
                match found {
                    Some(m) => m.name.clone(),
                    None => {
                        let actions: Vec<&str> = many
                            .iter()
                            .filter_map(|m| m.tool.as_ref().and_then(|t| t.action.as_deref()))
                            .collect();
                        let msg = match wanted {
                            Some(a) => format!(
                                "`{name}` has no action `{a}`; use one of: {}",
                                actions.join(", ")
                            ),
                            None => {
                                format!("`{name}` needs an `action`: one of {}", actions.join(", "))
                            }
                        };
                        let d = Diagnostic::error(codes::INVALID_PARAMS, msg)
                            .path("action")
                            .with_fix(
                                format!("Set `action` to `{}`", actions[0]),
                                FixAction::Set {
                                    record: None,
                                    path: "action".into(),
                                    value: actions[0].into(),
                                },
                            );
                        return Ok(tool_error(&RpcError::from(d)));
                    }
                }
            }
        };
        Ok(match ep.call(&method, Value::Object(args)) {
            Ok(result) => tool_result(result),
            Err(e) => tool_error(&e),
        })
    }

    /// Handles a Streamable HTTP POST body. `header` looks up request headers
    /// (case-insensitive). Returns the status and the JSON body, if any.
    pub fn handle_http(
        &mut self,
        ep: &mut dyn Endpoint,
        header: &dyn Fn(&str) -> Option<String>,
        body: &[u8],
    ) -> (u16, Option<Value>) {
        let msg: Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(_) => {
                let parse = Incoming::parse("{").err();
                return (400, parse);
            }
        };
        let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
        let is_request = msg.get("id").is_some() && method.is_some();
        if let Some(method) = &method
            && let Some(problem) = header_problem(header, &msg, method)
        {
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            return (400, Some(jsonrpc::error_response(id, &problem)));
        }
        match self.handle(ep, msg) {
            None => (if is_request { 400 } else { 202 }, None),
            Some(resp) => {
                let code = resp.pointer("/error/code").and_then(Value::as_i64);
                let status = match code {
                    Some(jsonrpc::METHOD_NOT_FOUND) => 404,
                    Some(
                        UNSUPPORTED_VERSION
                        | HEADER_MISMATCH
                        | jsonrpc::PARSE_ERROR
                        | jsonrpc::INVALID_REQUEST,
                    ) => 400,
                    _ => 200,
                };
                (status, Some(resp))
            }
        }
    }
}

/// Checks the modern request headers against the body. Requests without
/// `MCP-Protocol-Version` are legacy and pass.
fn header_problem(
    header: &dyn Fn(&str) -> Option<String>,
    msg: &Value,
    method: &str,
) -> Option<RpcError> {
    let version = header("mcp-protocol-version")?;
    let mismatch = |what: String| {
        Some(rpc_error(
            HEADER_MISMATCH,
            format!("Header mismatch: {what}"),
            None,
        ))
    };
    if let Some(body_version) = msg
        .pointer(&format!("/params/_meta/{}", VERSION_KEY.replace('/', "~1")))
        .and_then(Value::as_str)
        && body_version != version
    {
        return mismatch(format!(
            "MCP-Protocol-Version `{version}` does not match `{body_version}`"
        ));
    }
    if version.as_str() < MODERN {
        return None;
    }
    match header("mcp-method").map(|m| decode(&m)) {
        Some(m) if m == method => {}
        Some(m) => return mismatch(format!("Mcp-Method `{m}` does not match `{method}`")),
        None => return mismatch("Mcp-Method is required".into()),
    }
    if method == "tools/call" {
        let name = msg
            .pointer("/params/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match header("mcp-name").map(|n| decode(&n)) {
            Some(n) if n == name => {}
            Some(n) => return mismatch(format!("Mcp-Name `{n}` does not match `{name}`")),
            None => return mismatch("Mcp-Name is required for tools/call".into()),
        }
    }
    None
}

/// Decodes the `=?base64?...?=` header sentinel; other values pass through.
fn decode(value: &str) -> String {
    let Some(b64) = value
        .strip_prefix("=?base64?")
        .and_then(|v| v.strip_suffix("?="))
    else {
        return value.to_owned();
    };
    let sextet = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let bits: Vec<u8> = b64.bytes().filter_map(sextet).collect();
    let mut out = Vec::new();
    for chunk in bits.chunks(4) {
        let n =
            chunk.iter().fold(0u32, |acc, &b| (acc << 6) | u32::from(b)) << (6 * (4 - chunk.len()));
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A successful tool result: the method's `text` (or pretty JSON) plus the result
/// as structured content.
pub fn tool_result(result: Value) -> Value {
    let text = match result.get("text").and_then(Value::as_str) {
        Some(t) => t.to_owned(),
        None => serde_json::to_string_pretty(&result).unwrap_or_default(),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": result,
        "isError": false,
    })
}

/// A tool error: rendered diagnostics plus their JSON (with fixes).
pub fn tool_error(e: &RpcError) -> Value {
    let diags = e.diagnostics();
    let json = serde_json::to_string(&diags).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": format!("{e}\n\ndiagnostics: {json}") }],
        "structuredContent": { "ok": false, "diagnostics": diags },
        "isError": true,
    })
}

/// Serves MCP over stdio until end of input: one JSON-RPC message per line.
pub fn serve_stdio(
    ep: &mut dyn Endpoint,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> io::Result<()> {
    let mut adapter = Adapter::new();
    let mut input = input;
    let mut output = output;
    while let Some(line) = lines::read_line(&mut input)? {
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => adapter.handle(ep, msg),
            Err(_) => Incoming::parse(&line).err(),
        };
        if let Some(r) = reply {
            lines::write_message(&mut output, &r)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_header_sentinels_decode() {
        assert_eq!(decode("=?base64?SGVsbG8sIOS4lueVjA==?="), "Hello, 世界");
        assert_eq!(decode("ara_query"), "ara_query");
    }
}
