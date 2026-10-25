//! JSON-RPC 2.0 over a localhost WebSocket, polled without blocking. Both ends use
//! it: the owner accepts players, players connect back (they never listen).
//! Windows socket timeouts are coarse (~15.6 ms), so callers poll and sleep briefly
//! instead of blocking reads with timeouts.
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    io::ErrorKind,
    net::TcpStream,
    time::{Duration, Instant},
};
use tungstenite::{Message as Frame, WebSocket};
mod pump;
pub use pump::Pump;

/// A decoded JSON-RPC message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A call expecting a response.
    Request {
        /// Request ID.
        id: Value,
        /// Method name.
        method: String,
        /// Params (`null` when absent).
        params: Value,
    },
    /// A fire-and-forget message.
    Notification {
        /// Method name.
        method: String,
        /// Params.
        params: Value,
    },
    /// A response to one of our requests.
    Response {
        /// Matching request ID.
        id: Value,
        /// Result, or the error object.
        result: Result<Value, Value>,
    },
}

/// Link failures; `Closed` means the peer went away.
#[derive(Debug)]
pub enum LinkError {
    /// The connection closed.
    Closed,
    /// Transport or framing failure.
    Io(String),
    /// A response did not arrive in time.
    Timeout,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("link closed"),
            Self::Io(e) => write!(f, "link: {e}"),
            Self::Timeout => f.write_str("link timeout"),
        }
    }
}
impl std::error::Error for LinkError {}

fn io(e: tungstenite::Error) -> LinkError {
    match e {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            LinkError::Closed
        }
        e => LinkError::Io(e.to_string()),
    }
}

/// One non-blocking JSON-RPC connection.
pub struct Link {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

impl Link {
    /// Connects to the owner at `127.0.0.1:port` (blocking handshake), then switches
    /// to non-blocking polling.
    pub fn connect(port: u16) -> Result<Self, LinkError> {
        let stream =
            TcpStream::connect(("127.0.0.1", port)).map_err(|e| LinkError::Io(e.to_string()))?;
        stream
            .set_nodelay(true)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        let (ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/play"), stream)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        Self::nonblocking(ws)
    }

    /// Accepts a player connection (blocking handshake), then switches to polling.
    /// Accepted sockets may inherit a listener's non-blocking mode; it is reset first.
    pub fn accept(stream: TcpStream) -> Result<Self, LinkError> {
        stream
            .set_nonblocking(false)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| LinkError::Io(e.to_string()))?;
        stream
            .set_nodelay(true)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        let ws = tungstenite::accept(stream).map_err(|e| LinkError::Io(e.to_string()))?;
        Self::nonblocking(ws)
    }

    fn nonblocking(ws: WebSocket<TcpStream>) -> Result<Self, LinkError> {
        ws.get_ref()
            .set_nonblocking(true)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        Ok(Self { ws, next_id: 1 })
    }

    /// Reads one message if available.
    pub fn poll(&mut self) -> Result<Option<Message>, LinkError> {
        loop {
            match self.ws.read() {
                Ok(Frame::Text(text)) => return parse(text.as_str()).map(Some),
                Ok(Frame::Close(_)) => return Err(LinkError::Closed),
                Ok(_) => continue,
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    self.flush()?;
                    return Ok(None);
                }
                Err(e) => return Err(io(e)),
            }
        }
    }

    /// Waits up to `timeout` for the next message, polling every ~0.5 ms.
    pub fn wait(&mut self, timeout: Duration) -> Result<Option<Message>, LinkError> {
        let start = Instant::now();
        loop {
            if let Some(message) = self.poll()? {
                return Ok(Some(message));
            }
            if start.elapsed() >= timeout {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    /// Sends a request; returns its numeric ID.
    pub fn request(&mut self, method: &str, params: impl Serialize) -> Result<u64, LinkError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        Ok(id)
    }

    /// Sends a notification.
    pub fn notify(&mut self, method: &str, params: impl Serialize) -> Result<(), LinkError> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Answers request `id` with a result or a JSON-RPC error object.
    pub fn respond(&mut self, id: Value, result: Result<Value, Value>) -> Result<(), LinkError> {
        let message = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        self.send(&message)
    }

    fn send(&mut self, message: &Value) -> Result<(), LinkError> {
        match self.ws.send(Frame::text(message.to_string())) {
            Ok(()) => Ok(()),
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    /// Drives buffered writes (a non-blocking send may leave bytes queued).
    pub fn flush(&mut self) -> Result<(), LinkError> {
        match self.ws.flush() {
            Ok(()) => Ok(()),
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    /// Sends a request and waits for its response, handing other messages to
    /// `other` (notifications such as status heartbeats, or requests).
    pub fn call(
        &mut self,
        method: &str,
        params: impl Serialize,
        timeout: Duration,
        mut other: impl FnMut(Message),
    ) -> Result<Result<Value, Value>, LinkError> {
        let id = self.request(method, params)?;
        let start = Instant::now();
        loop {
            let left = timeout.saturating_sub(start.elapsed());
            match self.wait(left.min(Duration::from_millis(50)))? {
                Some(Message::Response { id: got, result }) if got == json!(id) => {
                    return Ok(result);
                }
                Some(message) => other(message),
                None if start.elapsed() >= timeout => return Err(LinkError::Timeout),
                None => {}
            }
        }
    }
}

/// An error object with a stable code, message and structured data.
pub fn error(code: i64, message: impl Into<String>, data: Value) -> Value {
    json!({"code": code, "message": message.into(), "data": data})
}

fn parse(text: &str) -> Result<Message, LinkError> {
    let value: Value = serde_json::from_str(text).map_err(|e| LinkError::Io(e.to_string()))?;
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    match (value.get("id").cloned(), method) {
        (Some(id), Some(method)) => Ok(Message::Request { id, method, params }),
        (None, Some(method)) => Ok(Message::Notification { method, params }),
        (Some(id), None) => {
            let result = match value.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            };
            Ok(Message::Response { id, result })
        }
        (None, None) => Err(LinkError::Io(format!("not a JSON-RPC message: {text}"))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_responses_round_trip_over_a_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let player = std::thread::spawn(move || {
            let mut link = Link::connect(port).unwrap();
            link.notify("play.status", json!({"tick": 1})).unwrap();
            loop {
                if let Some(Message::Request { id, method, params }) =
                    link.wait(Duration::from_secs(5)).unwrap()
                {
                    assert_eq!(method, "play.inspect");
                    link.respond(id, Ok(params)).unwrap();
                    link.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(50));
                    return;
                }
            }
        });
        let mut owner = Link::accept(listener.accept().unwrap().0).unwrap();
        let mut notes = Vec::new();
        let result = owner
            .call(
                "play.inspect",
                json!({"entity": "e_1"}),
                Duration::from_secs(5),
                |m| notes.push(m),
            )
            .unwrap();
        assert_eq!(result, Ok(json!({"entity": "e_1"})));
        assert!(
            matches!(&notes[0], Message::Notification { method, .. } if method == "play.status")
        );
        player.join().unwrap();
    }
}
