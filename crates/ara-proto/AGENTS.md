# ara-proto

The Engine Protocol wire format (decision A5): JSON-RPC 2.0 messages, the scope
vocabulary, the method catalog and the `Endpoint` trait. No IO beyond `std::io`;
the host is `ara-host`, transports are `ara-rpc`, MCP is `ara-mcp`.
Depends on ara-diag, serde.

## Public API

- `jsonrpc`: `Incoming::{Request, Notification, Response}` (`parse`, `from_value`;
  a parse failure *is* the error response to send), builders `request`,
  `notification`, `response`, `error_response`, `reply`; codes `PARSE_ERROR`,
  `INVALID_REQUEST`, `METHOD_NOT_FOUND`, `INVALID_PARAMS`, `ENGINE_ERROR`
  (-32000), `UNAUTHORIZED` (-32001).
- `RpcError { code, message, data }`: `from_diagnostics`, `diagnostics()`,
  `From<Diagnostic>`, `Display` (rustc-style). Engine errors always carry
  `data.diagnostics`.
- `Scope`: `read`, `edit:scene`, `edit:assets`, `edit:code`, `play`, `run`,
  `import`, `bake`, `build`, `generate:$`, `fs:outside-project`, `net`.
  `Scope::ALL`, `as_str`, `doc`, `FromStr` (did-you-mean), `agent_default()`
  (all but `generate:$`, `fs:outside-project`, `net`).
- `MethodInfo { name, summary, scope, hints, tool, params, service }`: one
  `rpc.discover` entry; `params` is JSON Schema 2020-12. `ToolBinding { tool,
  action }` says which MCP tool it backs. `MethodHints { readOnly, destructive,
  idempotent }`.
- `Endpoint::call(method, params) -> Result<Value, RpcError>`: implemented by
  `ara_host::Host` (in-process) and `ara_rpc::Client` (daemon).
- `ConnId` (`ConnId::LOCAL` = in-process), `PROTOCOL` (`ara-ep/1`).
- `lines::{read_line, write_message}`: newline-delimited JSON for stdio.

## Rules

- JSON-RPC error codes are derived from the first diagnostic's `ARA` code; add a
  mapping in `RpcError::from_diagnostics` only for codes JSON-RPC itself defines.
- Bump `PROTOCOL` only for incompatible wire changes; new methods are additive.
- Keep this crate free of engine types (no ara-doc/ara-ops): clients such as the
  editor and the MCP adapter depend on it alone.

## Testing

`cargo test -p ara-proto`: message round trips, malformed messages, scope parsing.
