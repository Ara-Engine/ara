# ara-rpc

Engine Protocol transports: a localhost WebSocket + HTTP server and stdio, all
feeding one handler thread; a blocking WebSocket client; the daemon discovery
file and the project token. Knows nothing about documents.
Depends on ara-proto, ara-diag, tungstenite (blocking, no async runtime).

## Public API

- `Handler { message(conn, text) -> Option<String>, http(&req) -> HttpResponse,
  closed(conn), tick(), outbox() -> Vec<(ConnId, String)>, stopping() }`.
- `serve(&mut dyn Handler, ServeOptions { listener, token, stdio, tick })`: runs
  until `stopping()`, stdin EOF (with `stdio`) or all sources are gone.
  Conn IDs: `0` in-process (`ConnId::LOCAL`), `1` stdio (`STDIO`), `2..` sockets.
- `Client::connect(port, token)`, `Client::for_project(root)` (via
  `.ara/daemon.json`); `impl Endpoint`; `notifications()`, `wait_notification(t)`.
- `DaemonInfo { pid, port, token, protocol }`: `read/write/remove(root)`, `url()`.
  `remove` only deletes a file naming this process.
- `project_token(root)`: `.ara/token`, created on first use (128 random bits).
- `http::{HttpRequest, HttpResponse, read_request, write_response, peek_head}`.

## Security

- Bind `127.0.0.1` only (the caller creates the listener).
- Every WebSocket upgrade and HTTP request needs the token: `Authorization:
  Bearer <token>` or `?token=`. Otherwise 401 with an ARA1807 JSON-RPC error body.
- A present `Origin` header must be localhost (DNS-rebinding guard), else 403.

## Gotchas

- WebSocket threads poll with a 20 ms read timeout so they can also write queued
  notifications; tungstenite resumes partial frames after a timeout.
- HTTP is one request per connection (`Connection: close`), `Content-Length`
  bodies only (16 MiB cap). Enough for MCP Streamable HTTP with JSON responses.
- Shutdown flushes queued WebSocket replies through bounded writer acknowledgments
  before returning, so daemon process exit cannot discard its shutdown response.
- Handler calls are serialized on the serve thread: a slow method blocks every
  connection. Long work belongs in jobs.

## Testing

`cargo test -p ara-rpc`: an echo handler behind a real listener (calls,
notifications, token and origin refusals, HTTP forwarding).
