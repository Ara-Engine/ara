//! Transport for the live link: JSON-RPC 2.0 in WebSocket text frames over localhost TCP (the A5
//! Engine Protocol shape), plus WebSocket binary frames for bulk data ("blobs"). A blob frame is
//! `blob id (u64 LE) | payload`; a JSON-RPC message refers to it by id, and is always sent after it.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Bytes, Message, WebSocket};

/// Protocol name and version, checked in the handshake.
pub const PROTOCOL: &str = "ara.play/0";

/// A typed JSON-RPC method: the name plus its parameter and result types.
pub trait Method {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned;
    type Output: Serialize + DeserializeOwned;
}

/// A structured JSON-RPC error. `data` carries machine-readable detail (errors are data).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Invalid params or a failed op (JSON-RPC "invalid params" range).
    pub fn invalid(err: &anyhow::Error) -> Self {
        Self {
            code: -32602,
            message: format!("{err:#}"),
            data: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Envelope {
    jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

/// One decoded JSON-RPC message.
#[derive(Debug)]
pub enum Incoming {
    Request {
        id: u64,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
}

impl Incoming {
    /// Decodes typed params of a request or notification.
    pub fn params<M: Method>(params: Value) -> Result<M::Params> {
        serde_json::from_value(params).with_context(|| format!("bad params for {}", M::NAME))
    }
}

fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(1 << 30))
        .max_frame_size(Some(1 << 30))
}

/// One end of a live link. Works in blocking mode (with an optional read timeout) or in
/// non-blocking mode, where [`Link::poll`] returns `Ok(None)` when nothing is ready.
pub struct Link {
    ws: WebSocket<TcpStream>,
    blobs: HashMap<u64, Bytes>,
    next_id: u64,
}

impl Link {
    fn new(ws: WebSocket<TcpStream>) -> Self {
        Self {
            ws,
            blobs: HashMap::new(),
            next_id: 1,
        }
    }

    /// Server side: completes the WebSocket handshake on an accepted connection.
    pub fn accept(stream: TcpStream) -> Result<Self> {
        stream.set_nodelay(!nagle())?;
        let ws = tungstenite::accept_with_config(stream, Some(config()))
            .map_err(|e| anyhow!("websocket accept: {e}"))?;
        Ok(Self::new(ws))
    }

    /// Client side: connects to `ws://host:port/path`.
    pub fn connect(url: &str) -> Result<Self> {
        let addr = url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| anyhow!("bad url {url}"))?;
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(!nagle())?;
        let (ws, _) = tungstenite::client::client_with_config(url, stream, Some(config()))
            .map_err(|e| anyhow!("websocket connect: {e}"))?;
        Ok(Self::new(ws))
    }

    pub fn set_nonblocking(&self, on: bool) -> Result<()> {
        Ok(self.ws.get_ref().set_nonblocking(on)?)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        Ok(self.ws.get_ref().set_read_timeout(timeout)?)
    }

    fn send(&mut self, msg: Message) -> Result<()> {
        let mut result = self.ws.send(msg);
        // A non-blocking socket may accept only part of a large frame; keep flushing.
        while let Err(tungstenite::Error::Io(e)) = &result {
            if e.kind() != ErrorKind::WouldBlock {
                break;
            }
            std::thread::sleep(Duration::from_micros(50));
            result = self.ws.flush();
        }
        result.map_err(|e| anyhow!("send: {e}"))
    }

    fn send_env(&mut self, env: &Envelope) -> Result<()> {
        let text = serde_json::to_string(env)?;
        self.send(Message::text(text))
    }

    /// Sends a request and returns its id; read the response with [`Link::poll`].
    pub fn request<M: Method>(&mut self, params: &M::Params) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_env(&Envelope {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: Some(M::NAME.into()),
            params: Some(serde_json::to_value(params)?),
            result: None,
            error: None,
        })?;
        Ok(id)
    }

    /// Sends a notification (no response).
    pub fn notify<M: Method>(&mut self, params: &M::Params) -> Result<()> {
        self.send_env(&Envelope {
            jsonrpc: "2.0".into(),
            id: None,
            method: Some(M::NAME.into()),
            params: Some(serde_json::to_value(params)?),
            result: None,
            error: None,
        })
    }

    /// Answers request `id` with a typed result or an error.
    pub fn respond<M: Method>(
        &mut self,
        id: u64,
        result: Result<M::Output, RpcError>,
    ) -> Result<()> {
        let (result, error) = match result {
            Ok(out) => (Some(serde_json::to_value(out)?), None),
            Err(err) => (None, Some(err)),
        };
        self.send_env(&Envelope {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: None,
            params: None,
            result,
            error,
        })
    }

    /// Reserves a blob id and returns a frame buffer that already holds its header, so callers
    /// can encode bulk data in place without a second copy.
    pub fn blob_frame(&mut self) -> (u64, Vec<u8>) {
        let id = self.next_id;
        self.next_id += 1;
        (id, id.to_le_bytes().to_vec())
    }

    /// Sends a frame from [`Link::blob_frame`].
    pub fn send_blob(&mut self, frame: Vec<u8>) -> Result<()> {
        self.send(Message::binary(frame))
    }

    /// Removes a received blob.
    pub fn take_blob(&mut self, id: u64) -> Option<Bytes> {
        self.blobs.remove(&id)
    }

    /// Reads the next JSON-RPC message, stashing blobs on the way. Returns `Ok(None)` when no
    /// complete message is ready (non-blocking or timed out); errors mean the link is gone.
    pub fn poll(&mut self) -> Result<Option<Incoming>> {
        loop {
            let msg = match self.ws.read() {
                Ok(msg) => msg,
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    return Ok(None);
                }
                Err(e) => bail!("link closed: {e}"),
            };
            match msg {
                Message::Text(text) => return parse(text.as_str()).map(Some),
                Message::Binary(data) => {
                    if data.len() < 8 {
                        bail!("blob frame without header");
                    }
                    let id = u64::from_le_bytes(data[..8].try_into()?);
                    self.blobs.insert(id, data.slice(8..));
                }
                Message::Close(_) => bail!("link closed by peer"),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }
}

fn parse(text: &str) -> Result<Incoming> {
    let env: Envelope = serde_json::from_str(text)?;
    if env.jsonrpc != "2.0" {
        bail!("not JSON-RPC 2.0");
    }
    Ok(match (env.id, env.method) {
        (Some(id), Some(method)) => Incoming::Request {
            id,
            method,
            params: env.params.unwrap_or(Value::Null),
        },
        (None, Some(method)) => Incoming::Notification {
            method,
            params: env.params.unwrap_or(Value::Null),
        },
        (Some(id), None) => Incoming::Response {
            id,
            result: match env.error {
                Some(err) => Err(err),
                None => Ok(env.result.unwrap_or(Value::Null)),
            },
        },
        (None, None) => bail!("message has neither id nor method"),
    })
}

/// `ARA_S5_NAGLE=1` leaves Nagle's algorithm on, to measure what TCP_NODELAY buys.
pub fn nagle() -> bool {
    std::env::var_os("ARA_S5_NAGLE").is_some_and(|v| v == "1")
}
