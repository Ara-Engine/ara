//! Authenticated Engine Protocol + MCP IO; the main thread retains the authority.
#[cfg(test)]
mod tests;
use crate::Editor;
use ara_proto::{ConnId, Incoming, jsonrpc};
use ara_rpc::{
    DaemonInfo, Handler, ServeOptions,
    http::{HttpRequest, HttpResponse},
};
use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    time::Duration,
};
enum Request {
    Message(ConnId, String, SyncSender<Option<String>>),
    Http(HttpRequest, SyncSender<HttpResponse>),
    Closed(ConnId),
}
struct Proxy {
    send: Sender<Request>,
    notifications: Receiver<(ConnId, String)>,
    stop: Arc<AtomicBool>,
}
impl Handler for Proxy {
    fn message(&mut self, conn: ConnId, text: &str) -> Option<String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send
            .send(Request::Message(conn, text.into(), tx))
            .ok()?;
        rx.recv().ok().flatten()
    }
    fn http(&mut self, req: &HttpRequest) -> HttpResponse {
        let (tx, rx) = mpsc::sync_channel(1);
        if self.send.send(Request::Http(req.clone(), tx)).is_err() {
            return HttpResponse::empty(503);
        }
        rx.recv().unwrap_or_else(|_| HttpResponse::empty(503))
    }
    fn closed(&mut self, conn: ConnId) {
        let _ = self.send.send(Request::Closed(conn));
    }
    fn outbox(&mut self) -> Vec<(ConnId, String)> {
        self.notifications.try_iter().collect()
    }
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}
pub(crate) struct Server {
    requests: Receiver<Request>,
    notifications: Sender<(ConnId, String)>,
    stop: Arc<AtomicBool>,
    root: PathBuf,
    adapter: ara_mcp::Adapter,
    pub port: u16,
}
impl Server {
    pub fn start(root: &Path) -> anyhow::Result<Self> {
        let token = ara_rpc::project_token(root)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        DaemonInfo {
            pid: std::process::id(),
            port,
            token: token.clone(),
            protocol: ara_proto::PROTOCOL.into(),
        }
        .write(root)?;
        let (send, requests) = mpsc::channel();
        let (notifications, recv) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut proxy = Proxy {
                send,
                notifications: recv,
                stop: thread_stop,
            };
            if let Err(e) = ara_rpc::serve(
                &mut proxy,
                ServeOptions {
                    listener: Some(listener),
                    token,
                    stdio: false,
                    tick: Duration::from_millis(5),
                },
            ) {
                eprintln!("editor protocol transport: {e}");
            }
        });
        Ok(Self {
            requests,
            notifications,
            stop,
            root: root.into(),
            adapter: ara_mcp::Adapter::new(),
            port,
        })
    }
    pub fn pump(&mut self, editor: &mut Editor) {
        let start = std::time::Instant::now();
        for _ in 0..16 {
            let Ok(request) = self.requests.try_recv() else {
                break;
            };
            match request {
                Request::Message(conn, text, reply) => {
                    let result = match Incoming::parse(&text) {
                        Ok(Incoming::Request { id, method, params }) => {
                            Some(jsonrpc::reply(id, editor.call_for(conn, &method, params)))
                        }
                        Ok(Incoming::Notification { method, params }) => {
                            let _ = editor.call_for(conn, &method, params);
                            None
                        }
                        Ok(Incoming::Response { .. }) => None,
                        Err(error) => Some(error),
                    };
                    let _ = reply.send(result.map(|v| v.to_string()));
                }
                Request::Http(req, reply) => {
                    let response = if req.path != "/mcp" {
                        HttpResponse::empty(404)
                    } else if req.method != "POST" {
                        HttpResponse::empty(405)
                    } else {
                        let header = |name: &str| req.header(name).map(str::to_owned);
                        match self.adapter.handle_http(editor, &header, &req.body) {
                            (status, Some(body)) => HttpResponse::json(status, &body),
                            (status, None) => HttpResponse::empty(status),
                        }
                    };
                    let _ = reply.send(response);
                }
                Request::Closed(conn) => editor.host.closed(conn),
            }
            if start.elapsed() > Duration::from_millis(1) {
                break;
            }
        }
        for (conn, value) in editor.host.drain_outbox() {
            if conn == ConnId::LOCAL {
                editor.panels_revision += 1;
                if value["method"] == "history" {
                    editor.review_revision += 1;
                }
            } else {
                let _ = self.notifications.send((conn, value.to_string()));
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        DaemonInfo::remove(&self.root);
    }
}
