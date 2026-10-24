//! One MCP connection, many registered projects. Each project keeps its own adapter/session.
use crate::{Adapter, complete, tool_error, tool_result};
use ara_proto::{Endpoint, Incoming, RpcError, jsonrpc, lines};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
};

/// Global stdio adapter backed by a user-level Hub endpoint.
#[derive(Default)]
pub struct GlobalAdapter {
    protocol: Adapter,
    projects: BTreeMap<String, Adapter>,
    sessions: BTreeMap<String, String>,
    bound: Option<String>,
    client: Option<Value>,
}

struct ProjectEndpoint<'a> {
    hub: &'a mut dyn Endpoint,
    project: &'a str,
}
impl Endpoint for ProjectEndpoint<'_> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.hub.call(
            "hub.call",
            json!({"project":self.project,"method":method,"params":params}),
        )
    }
}

fn invalid(message: &str) -> RpcError {
    ara_diag::Diagnostic::error(ara_diag::codes::INVALID_PARAMS, message).into()
}

impl GlobalAdapter {
    /// A new global MCP client; no project is selected until explicitly bound or targeted.
    pub fn new() -> Self {
        Self::default()
    }

    fn tool_list(&mut self, hub: &mut dyn Endpoint) -> Result<Value, RpcError> {
        let value = hub.call("hub.catalog", json!({"project":self.bound}))?;
        let methods: Vec<ara_proto::MethodInfo> = serde_json::from_value(value["methods"].clone())
            .map_err(|_| invalid("Hub returned an invalid catalog"))?;
        let mut tools = crate::tools::definitions(&methods, true);
        for tool in &mut tools {
            tool["inputSchema"]["properties"]["project"] = json!({"type":"string","description":"Hub-registered project ID, name or path. Optional after ara_session open or with a bound session."});
            if tool["name"] == "ara_session" {
                let actions = tool["inputSchema"]["properties"]["action"]["enum"]
                    .as_array_mut()
                    .ok_or_else(|| invalid("session catalog has no actions"))?;
                actions.insert(0, json!("open"));
                tool["description"] = "Bind to a Hub project with action: open, project: <name or path>. Other session/history actions operate on that project. Agent changes apply immediately and each turn can be undone.".into();
            }
        }
        tools.insert(0,json!({"name":"ara_projects","title":"Ara projects","description":"Manage Hub projects and engine versions. checkReleases refreshes the signed feed; available lists releases. installEngine returns an asynchronous job: poll downloads or cancelDownload. Engines install side by side; uninstallEngine rejects open project pins.","inputSchema":{"type":"object","additionalProperties":false,"properties":{
            "action":{"type":"string","enum":["list","create","open","close","status","engines","checkReleases","available","installEngine","uninstallEngine","downloads","cancelDownload"]},
            "project":{"type":"string","description":"Registered project ID, unique name or path."},
            "template":{"type":"string","description":"Template ID, e.g. blank; see templates returned by list."},
            "name":{"type":"string"},"path":{"type":"string"},"headless":{"type":"boolean","default":true},
            "version":{"type":"string","description":"Published engine version for installEngine or uninstallEngine."},
            "channel":{"type":"string","enum":["stable","preview"],"description":"Release channel for available; preview includes stable."},
            "job":{"type":"string","description":"Download job ID for cancelDownload."}
        },"required":["action"]}}));
        Ok(complete(
            json!({"tools":tools,"ttlMs":5000,"cacheScope":"private"}),
        ))
    }

    fn call_tool(
        &mut self,
        hub: &mut dyn Endpoint,
        name: &str,
        mut args: Map<String, Value>,
    ) -> Result<Value, RpcError> {
        if name == "ara_projects" {
            let action = args
                .remove("action")
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| invalid("ara_projects needs an action"))?;
            let method = match action.as_str() {
                "list" => "hub.projects",
                "create" => "hub.create",
                "open" => "hub.open",
                "close" => "hub.close",
                "status" => "hub.status",
                "engines" => "hub.engines",
                "checkReleases" => "hub.releases.check",
                "available" => "hub.engines.available",
                "installEngine" => "hub.engine.install",
                "uninstallEngine" => "hub.engine.uninstall",
                "downloads" => "hub.downloads",
                "cancelDownload" => "hub.download.cancel",
                _ => return Err(invalid("unknown ara_projects action")),
            };
            if action == "open" {
                args.entry("headless").or_insert(json!(true));
            }
            let mut result = hub.call(method, Value::Object(args))?;
            if action == "list" {
                result["templates"] = hub.call("hub.templates", json!({}))?["templates"].clone();
            }
            return Ok(complete(tool_result(result)));
        }
        let explicit = args
            .remove("project")
            .map(|value| {
                value
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        invalid("project must be a nonempty registered name, ID or path")
                    })
            })
            .transpose()?;
        let session = args
            .get("session")
            .map(|value| {
                value
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("session must be a session handle"))
            })
            .transpose()?;
        let session_project = session.as_ref().and_then(|s| self.sessions.get(s)).cloned();
        let requested = explicit.or_else(||session_project.clone()).or_else(||self.bound.clone()).ok_or_else(|| invalid("choose a registered project with project, or ara_session {action: open, project}"))?;
        let resolved = hub.call("hub.resolve", json!({"project":requested}))?;
        let project = resolved["id"]
            .as_str()
            .ok_or_else(|| invalid("Hub returned no project ID"))?
            .to_owned();
        if session_project.is_some_and(|bound| bound != project) {
            return Err(invalid("session belongs to a different project"));
        }
        if session.is_some()
            && !session
                .as_ref()
                .is_some_and(|s| self.sessions.contains_key(s))
        {
            return Err(invalid(
                "session was not opened by this global MCP connection",
            ));
        }
        let open = name == "ara_session" && args.get("action").is_some_and(|a| a == "open");
        if open {
            args.insert("action".into(), json!("begin"));
            args.entry("client").or_insert_with(|| {
                self.client
                    .as_ref()
                    .and_then(|v| v.get("name"))
                    .cloned()
                    .unwrap_or_else(|| json!("mcp-client"))
            });
        }
        let adapter = self
            .projects
            .entry(project.clone())
            .or_insert_with(Adapter::per_connection);
        if let Some(info) = &self.client {
            let _ = adapter.handle(
                &mut ProjectEndpoint {
                    hub,
                    project: &project,
                },
                json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"clientInfo":info}}),
            );
        }
        adapter.catalog = None;
        let result = adapter.call_tool(
            &mut ProjectEndpoint {
                hub,
                project: &project,
            },
            name,
            args,
        )?;
        if result["isError"] != true {
            if open {
                self.bound = Some(project.clone());
            }
            if let Some(session) = result["structuredContent"]["session"].as_str() {
                self.sessions.insert(session.to_owned(), project);
            }
        }
        Ok(complete(result))
    }

    /// Process one MCP request, preserving legacy and modern protocol negotiation.
    pub fn handle(&mut self, hub: &mut dyn Endpoint, message: Value) -> Option<Value> {
        let request = match Incoming::from_value(message.clone()) {
            Ok(Incoming::Request { id, method, params }) => (id, method, params),
            Ok(_) => return None,
            Err(error) => return Some(error),
        };
        let (id, method, params) = request;
        if let Some(info) = params
            .get("clientInfo")
            .or_else(|| params.pointer("/_meta/io.modelcontextprotocol~1clientInfo"))
        {
            self.client = Some(info.clone());
        }
        let modern = params
            .pointer("/_meta/io.modelcontextprotocol~1protocolVersion")
            .and_then(Value::as_str);
        if modern.is_some_and(|v| !crate::SUPPORTED.contains(&v)) {
            return self.protocol.handle(hub, message);
        }
        let result = match method.as_str() {
            "tools/list" => self.tool_list(hub),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default();
                let args = params["arguments"].as_object().cloned().unwrap_or_default();
                self.call_tool(hub, name, args)
                    .or_else(|e| Ok(complete(tool_error(&e))))
            }
            _ => {
                let mut reply = self.protocol.handle(hub, message)?;
                if let Some(result) = reply.get_mut("result")
                    && result.get("instructions").is_some()
                {
                    result["instructions"] = "Ara Hub: call ara_projects list to see registered projects. Target any tool with project, or bind using ara_session {action: open, project}. Agent changes apply immediately; undo per turn and overwrite notices are available. Only Hub-registered projects are visible.".into();
                }
                return Some(reply);
            }
        };
        Some(jsonrpc::reply(id, result))
    }
}

/// Global stdio server. Engine session state remains isolated per MCP connection and project.
pub fn serve_global_stdio(
    hub: &mut dyn Endpoint,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<()> {
    let mut adapter = GlobalAdapter::new();
    while let Some(line) = lines::read_line(input)? {
        let reply = match serde_json::from_str(&line) {
            Ok(message) => adapter.handle(hub, message),
            Err(_) => Incoming::parse(&line).err(),
        };
        if let Some(reply) = reply {
            lines::write_message(output, &reply)?;
        }
    }
    Ok(())
}
