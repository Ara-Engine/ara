# ara-mcp

The MCP adapter: thirteen tools generated from the Engine Protocol catalog, served
over stdio (`ara mcp`) and Streamable HTTP (`POST /mcp` on the daemon). It holds no
engine state; every call goes to an `ara_proto::Endpoint`.
Depends on ara-proto, ara-diag.

## Public API

- `Adapter::new()` (stateless: Streamable HTTP) and `Adapter::per_connection()`
  (one client, stdio: keeps that client's session, so `session` is optional);
  `handle(&mut dyn Endpoint, msg) -> Option<Value>` (one JSON-RPC message),
  `tools(ep)`, `call_tool(ep, name, args)`,
  `handle_http(ep, header, body) -> (status, Option<body>)`.
- `serve_stdio(ep, input, output)`: newline-delimited JSON until EOF, per
  connection.
- `tools::TOOLS` (name, title, description, reserved method family),
  `tools::definitions(catalog, optional_session)`, `tools::methods_of(catalog, tool)`.
- `tool_result(value)`, `tool_error(&RpcError)`; `SUPPORTED`, `MODERN`.

## Tools (fixed order)

`ara_query`, `ara_inspect`, `ara_schema`, `ara_apply`, `ara_run`, `ara_capture`,
`ara_play`, `ara_test`, `ara_logs`, `ara_why`, `ara_session`, `ara_docs`, `ara_jobs`.

- `ara_logs` returns the bus grouped by code within a character `budget`;
  `ara_why` (the `why` method) returns ranked causes with runnable fixes. The
  server instructions tell agents to ask `ara_why` first when something is wrong.
- Claude Code gives the model `structuredContent`, not `content`: a result's JSON
  is what agents pay for, so methods meant for agents keep it small (`why`, `logs`
  answer with their text unless asked for more).

- A tool's methods are the catalog entries whose `tool` binding names it.
  One method without `action`: the tool's input schema *is* its params schema.
  Several: an `action` enum (in registration order) plus the union of their
  params; a property used by only some actions says so in its description.
- No methods yet (`ara_run`, `ara_capture`, `ara_play`, `ara_test` today): a
  placeholder schema; calls go to the reserved family (`play.<action>`) and the
  host answers ARA1806. Registering a service with bound methods lights the tool
  up on the next `tools/list` with no adapter change.
- Optional params (`anyOf [X, null]`, `default: null`) are simplified to `X`.

## MCP versions (2026-07-28, dual-era)

- Modern: stateless; per-request `_meta` `io.modelcontextprotocol/protocolVersion`
  (unsupported → -32022 with `supported`); `server/discover`; every result has
  `resultType: "complete"` and `_meta` serverInfo; `tools/list` has `ttlMs` and
  `cacheScope: "private"`. HTTP: `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name`
  must match the body (-32020, 400); unknown methods → 404; notifications → 202.
- Legacy: `initialize` (negotiates 2025-11-25 and older), `ping`; requests without
  `MCP-Protocol-Version` are treated as legacy.
- The session is an ordinary tool argument (`session` from `ara_session begin`).
  Per connection, a call without one uses the session the client began, else one
  begun on first use (client named after `clientInfo.name`, made author-safe:
  `Claude Code` → `agent:claude-code`); `session.end` of it clears it.

## Results

Success: `content` = the method's `text` field (scene-to-text) or pretty JSON;
`structuredContent` = the full result. Engine errors: `isError: true`, rendered
diagnostics plus their JSON (fixes included). Unknown tools are protocol errors
(-32602).

## Testing

`cargo test -p ara-mcp`: a fake endpoint serving `ara_host::core_methods()`;
per-connection sessions (optional `session`, one begin, reuse).
The `tools` snapshot is the whole generated `tools/list`; review its diff whenever
method params, doc comments or tool descriptions change.
