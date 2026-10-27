//! `ara serve` (the project daemon) and `ara mcp` (the MCP server).
//!
//! The daemon owns the project's host: one per project or worktree, guarded by
//! `.ara/lock`, discoverable through `.ara/daemon.json`. It serves the Engine
//! Protocol over WebSocket (and stdio with `--stdio`), and MCP Streamable HTTP at
//! `POST /mcp`, all on `127.0.0.1` with the project token.
//!
//! `ara mcp` speaks MCP on stdio. With a daemon running it is a thin client;
//! otherwise it becomes the daemon itself (so the CLI and other clients still get
//! a running engine) and serves MCP on stdio until stdin closes.

use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

use ara_host::{Host, HostConfig};
use ara_mcp::Adapter;
use ara_proto::{ConnId, PROTOCOL};
use ara_rpc::http::{HttpRequest, HttpResponse};
use ara_rpc::{Client, DaemonInfo, Handler, STDIO, ServeOptions, project_token};
use serde_json::Value;

/// The daemon's handler: Engine Protocol on sockets (and stdio for `serve
/// --stdio`), MCP on `/mcp` (and on stdio for `ara mcp`). The stdio client gets
/// its own per-connection adapter (optional `session`); `/mcp` is stateless.
struct Daemon {
    host: Host,
    http_mcp: Adapter,
    stdio_mcp: Adapter,
    mcp_on_stdio: bool,
}

impl Handler for Daemon {
    fn message(&mut self, conn: ConnId, text: &str) -> Option<String> {
        if conn == STDIO && self.mcp_on_stdio {
            let reply = match serde_json::from_str::<Value>(text) {
                Ok(msg) => self.stdio_mcp.handle(&mut self.host, msg),
                Err(_) => ara_proto::Incoming::parse(text).err(),
            };
            return reply.map(|r| r.to_string());
        }
        self.host.handle_text(conn, text)
    }

    fn http(&mut self, req: &HttpRequest) -> HttpResponse {
        if req.path != "/mcp" {
            return HttpResponse::empty(404);
        }
        if req.method != "POST" {
            return HttpResponse::empty(405);
        }
        let header = |name: &str| req.header(name).map(str::to_owned);
        match self
            .http_mcp
            .handle_http(&mut self.host, &header, &req.body)
        {
            (status, Some(body)) => HttpResponse::json(status, &body),
            (status, None) => HttpResponse::empty(status),
        }
    }

    fn closed(&mut self, conn: ConnId) {
        self.host.closed(conn);
    }

    fn tick(&mut self) {
        self.host.tick();
    }

    fn outbox(&mut self) -> Vec<(ConnId, String)> {
        self.host
            .drain_outbox()
            .into_iter()
            .map(|(c, v)| (c, v.to_string()))
            .collect()
    }

    fn stopping(&self) -> bool {
        self.host.stopping()
    }
}

/// Runs the daemon for `root` until it is shut down (or stdin closes, with
/// `stdio`). With `announce`, prints `{pid, port, url, mcp}` as one JSON line once
/// listening (never with stdio, where stdout carries protocol messages).
fn run_daemon(
    root: &Path,
    port: u16,
    stdio: bool,
    mcp_on_stdio: bool,
    announce: bool,
) -> Result<bool, String> {
    let mut host = Host::open(
        crate::project::load(root).map_err(|d| d.to_string())?,
        HostConfig::default(),
    )
    .map_err(|d| d.to_string())?;
    let root = host.root().to_path_buf();
    crate::assets::register(&mut host).map_err(|diagnostic| diagnostic.to_string())?;
    // "Why can't I hear X?" next to the built-in explainers.
    ara_audio_why::register(host.why());
    // "Why is this NPC stuck?": brain, perception and navigation causes.
    host.why().register(Box::new(ara_ai::why::NpcExplainer));
    // World cells, spatial queries and attributed multi-document cell moves.
    ara_world::service::register(&mut host).map_err(|d| d.to_string())?;
    // Play and test run in pooled ara-player processes owned by this daemon.
    host.register(Box::new(
        ara_play_host::PlayService::new(&root, ara_play_host::PlayConfig::for_project(&root))
            .with_headless_automation(),
    ))
    .map_err(|d| d.to_string())?;
    let token = project_token(&root).map_err(|e| format!("cannot create `.ara/token`: {e}"))?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let info = DaemonInfo {
        pid: std::process::id(),
        port,
        token: token.clone(),
        protocol: PROTOCOL.into(),
    };
    info.write(&root)
        .map_err(|e| format!("cannot write `.ara/daemon.json`: {e}"))?;
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");
    eprintln!(
        "ara: serving {} on {} (MCP: POST {mcp_url})",
        root.display(),
        info.url()
    );
    if announce && !stdio {
        let line =
            serde_json::json!({ "pid": info.pid, "port": port, "url": info.url(), "mcp": mcp_url });
        println!("{line}");
    }
    let mut daemon = Daemon {
        host,
        http_mcp: Adapter::new(),
        stdio_mcp: Adapter::per_connection(),
        mcp_on_stdio,
    };
    let opts = ServeOptions {
        listener: Some(listener),
        token,
        stdio,
        tick: Duration::from_millis(50),
    };
    let result = ara_rpc::serve(&mut daemon, opts);
    daemon.host.flush();
    DaemonInfo::remove(&root);
    result.map_err(|e| e.to_string())?;
    Ok(true)
}

/// `ara serve`.
pub fn serve(root: &Path, port: Option<u16>, stdio: bool, json: bool) -> Result<bool, String> {
    run_daemon(root, port.unwrap_or(0), stdio, false, json)
}

/// `ara mcp`.
pub fn mcp(root: &Path) -> Result<bool, String> {
    if let Some(mut client) = Client::for_project(root) {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let mut output = io::stdout().lock();
        ara_mcp::serve_stdio(&mut client, &mut input, &mut output).map_err(|e| e.to_string())?;
        return Ok(true);
    }
    run_daemon(root, 0, true, true, false)
}
