//! The adapter over a fake endpoint serving the real core catalog: the generated
//! tool list (snapshot), routing, tool errors and the MCP versioning rules.

use ara_diag::{Diagnostic, codes};
use ara_mcp::{Adapter, tools};
use ara_proto::{Endpoint, MethodInfo, RpcError};
use serde_json::{Value, json};

struct Fake {
    catalog: Vec<MethodInfo>,
    calls: Vec<(String, Value)>,
}

impl Fake {
    fn new() -> Self {
        Fake {
            catalog: ara_host::core_methods(),
            calls: Vec::new(),
        }
    }
}

impl Endpoint for Fake {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls.push((method.to_owned(), params.clone()));
        match method {
            "rpc.discover" => Ok(json!({ "methods": self.catalog })),
            "session.begin" => Ok(json!({ "session": "ses_fake", "echo": params })),
            m if m.starts_with("play") => Err(RpcError::from(Diagnostic::error(
                codes::SERVICE_UNAVAILABLE,
                format!("`{m}` is not available yet"),
            ))),
            _ => Ok(json!({ "text": "e_flr1 Floor", "echo": params })),
        }
    }
}

fn request(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
}

fn modern(mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    params
}

fn call_tool(a: &mut Adapter, ep: &mut Fake, name: &str, args: Value) -> Value {
    let r = a
        .handle(
            ep,
            request(
                "tools/call",
                modern(json!({ "name": name, "arguments": args })),
            ),
        )
        .expect("a response");
    r["result"].clone()
}

#[test]
fn the_tool_list_is_generated_from_the_catalog() {
    let (mut a, mut ep) = (Adapter::new(), Fake::new());
    let r = a
        .handle(&mut ep, request("tools/list", modern(json!({}))))
        .expect("a response");
    let result = &r["result"];
    assert_eq!(result["resultType"], "complete");
    let tools = result["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    let expected: Vec<&str> = tools::TOOLS.iter().map(|t| t.name).collect();
    assert_eq!(names, expected, "fixed, deterministic order");
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
    }
    let pretty = serde_json::to_string_pretty(&result["tools"]).expect("serializes");
    insta::assert_snapshot!("tools", pretty);
}

#[test]
fn tools_route_to_methods_and_failures_are_tool_errors() {
    let (mut a, mut ep) = (Adapter::new(), Fake::new());
    let r = call_tool(
        &mut a,
        &mut ep,
        "ara_session",
        json!({ "action": "begin", "client": "claude-code" }),
    );
    assert_eq!(r["isError"], false);
    assert_eq!(
        ep.calls.last(),
        Some(&(
            "session.begin".to_owned(),
            json!({ "client": "claude-code" })
        ))
    );

    let r = call_tool(
        &mut a,
        &mut ep,
        "ara_query",
        json!({ "session": "ses_1", "q": "Light" }),
    );
    assert_eq!(
        r["content"][0]["text"], "e_flr1 Floor",
        "methods' text is the content"
    );
    assert_eq!(ep.calls.last().map(|c| c.0.as_str()), Some("query"));

    let r = call_tool(
        &mut a,
        &mut ep,
        "ara_play",
        json!({ "session": "ses_1", "action": "start" }),
    );
    assert_eq!(r["isError"], true);
    assert_eq!(r["structuredContent"]["diagnostics"][0]["code"], "ARA1806");
    assert_eq!(ep.calls.last().map(|c| c.0.as_str()), Some("play.start"));

    let r = call_tool(&mut a, &mut ep, "ara_session", json!({ "action": "begn" }));
    assert_eq!(r["isError"], true);
    let fix = &r["structuredContent"]["diagnostics"][0]["fixes"][0];
    assert_eq!(fix["action"], "set");

    let r = a
        .handle(
            &mut ep,
            request("tools/call", json!({ "name": "nope", "arguments": {} })),
        )
        .expect("a response");
    assert_eq!(
        r["error"]["code"], -32602,
        "unknown tools are protocol errors"
    );
}

#[test]
fn versions_are_checked_per_request_and_legacy_clients_still_work() {
    let (mut a, mut ep) = (Adapter::new(), Fake::new());
    let mut params = modern(json!({}));
    params["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("1900-01-01");
    let r = a
        .handle(&mut ep, request("tools/list", params))
        .expect("a response");
    assert_eq!(r["error"]["code"], -32022);
    assert_eq!(r["error"]["data"]["requested"], "1900-01-01");

    let r = a
        .handle(&mut ep, request("server/discover", modern(json!({}))))
        .expect("a response");
    assert_eq!(r["result"]["supportedVersions"][0], "2026-07-28");
    let r = a
        .handle(
            &mut ep,
            request("initialize", json!({ "protocolVersion": "2025-06-18" })),
        )
        .expect("a response");
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");

    let headers = |name: &str| match name {
        "mcp-protocol-version" => Some("2026-07-28".to_owned()),
        "mcp-method" => Some("tools/call".to_owned()),
        "mcp-name" => Some("ara_logs".to_owned()),
        _ => None,
    };
    let body = request(
        "tools/call",
        modern(json!({ "name": "ara_query", "arguments": {} })),
    );
    let (status, resp) = a.handle_http(&mut ep, &headers, body.to_string().as_bytes());
    assert_eq!(status, 400);
    assert_eq!(resp.expect("error body")["error"]["code"], -32020);
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": {} });
    assert_eq!(
        a.handle_http(&mut ep, &|_| None, note.to_string().as_bytes()),
        (202, None)
    );
}

#[test]
fn per_connection_adapters_make_the_session_optional() {
    let (mut a, mut ep) = (Adapter::per_connection(), Fake::new());
    let init = json!({ "protocolVersion": "2025-11-25", "clientInfo": { "name": "Claude Code", "version": "2" } });
    a.handle(&mut ep, request("initialize", init))
        .expect("initialized");
    let r = a
        .handle(&mut ep, request("tools/list", json!({})))
        .expect("a response");
    let apply = r["result"]["tools"]
        .as_array()
        .and_then(|t| t.iter().find(|t| t["name"] == "ara_apply"))
        .expect("ara_apply")
        .clone();
    assert!(
        !apply["inputSchema"]["required"]
            .as_array()
            .expect("required")
            .contains(&json!("session"))
    );

    // The first call without a session begins one, named after the client; later
    // calls reuse it, and a session the client begins replaces it.
    for (name, args) in [
        ("ara_query", json!({ "q": "Light" })),
        ("ara_logs", json!({})),
    ] {
        let legacy = request("tools/call", json!({ "name": name, "arguments": args }));
        a.handle(&mut ep, legacy).expect("a response");
    }
    let begun: Vec<&(String, Value)> = ep
        .calls
        .iter()
        .filter(|(m, _)| m == "session.begin")
        .collect();
    assert_eq!(begun.len(), 1);
    assert_eq!(begun[0].1["client"], "claude-code");
    let query = ep.calls.iter().find(|(m, _)| m == "query").expect("query");
    let logs = ep.calls.iter().find(|(m, _)| m == "logs").expect("logs");
    assert_eq!(query.1["session"], logs.1["session"]);
}
