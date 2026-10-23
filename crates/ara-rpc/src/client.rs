//! A blocking WebSocket client for the Engine Protocol. Notifications that arrive
//! while waiting for a response are buffered for [`Client::notifications`].

use std::collections::VecDeque;
use std::io;
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

use ara_diag::{Diagnostic, codes};
use ara_proto::{Endpoint, Incoming, RpcError, jsonrpc};
use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

use crate::DaemonInfo;

const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// A response: its `id` and outcome.
type Reply = (Value, Result<Value, RpcError>);

/// A connection to a daemon.
pub struct Client {
    ws: WebSocket<TcpStream>,
    next: u64,
    notes: VecDeque<Value>,
}

fn unreachable(msg: String) -> RpcError {
    RpcError::from(Diagnostic::error(codes::DAEMON_UNREACHABLE, msg))
}

impl Client {
    /// Connects to `127.0.0.1:port` with the project token.
    pub fn connect(port: u16, token: &str) -> io::Result<Client> {
        let stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut req = format!("ws://127.0.0.1:{port}/")
            .into_client_request()
            .map_err(io::Error::other)?;
        let auth = format!("Bearer {token}")
            .parse()
            .map_err(io::Error::other)?;
        req.headers_mut().insert("Authorization", auth);
        let (ws, _) = tungstenite::client(req, stream).map_err(io::Error::other)?;
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_millis(50)))?;
        Ok(Client {
            ws,
            next: 1,
            notes: VecDeque::new(),
        })
    }

    /// Connects to the daemon named by `<root>/.ara/daemon.json`; `None` if there is
    /// none or it does not answer.
    pub fn for_project(root: &Path) -> Option<Client> {
        let info = DaemonInfo::read(root)?;
        Client::connect(info.port, &info.token).ok()
    }

    /// Notifications received so far, oldest first.
    pub fn notifications(&mut self) -> Vec<Value> {
        self.notes.drain(..).collect()
    }

    /// Waits up to `timeout` for a notification.
    pub fn wait_notification(&mut self, timeout: Duration) -> Option<Value> {
        let end = Instant::now() + timeout;
        while self.notes.is_empty() && Instant::now() < end {
            if self.read_one().is_err() {
                break;
            }
        }
        self.notes.pop_front()
    }

    /// Reads one frame; returns a response if it was one.
    fn read_one(&mut self) -> Result<Option<Reply>, RpcError> {
        match self.ws.read() {
            Ok(Message::Text(text)) => match Incoming::parse(&text) {
                Ok(Incoming::Response { id, result }) => Ok(Some((id, result))),
                Ok(Incoming::Notification { method, params }) => {
                    self.notes
                        .push_back(json!({ "method": method, "params": params }));
                    Ok(None)
                }
                _ => Ok(None),
            },
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(unreachable(format!("daemon connection lost: {e}"))),
        }
    }
}

impl Endpoint for Client {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next;
        self.next += 1;
        let msg = jsonrpc::request(json!(id), method, params);
        self.ws
            .send(Message::text(msg.to_string()))
            .map_err(|e| unreachable(format!("cannot send to the daemon: {e}")))?;
        let end = Instant::now() + CALL_TIMEOUT;
        while Instant::now() < end {
            if let Some((rid, result)) = self.read_one()?
                && rid == json!(id)
            {
                return result;
            }
        }
        Err(unreachable(format!(
            "`{method}` got no answer in {}s",
            CALL_TIMEOUT.as_secs()
        )))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}
