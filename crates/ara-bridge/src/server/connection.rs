//! Bounded nonblocking client multiplexing; an idle agent cannot monopolize a bridge.
use super::State;
use crate::{BridgeError, Error, PROTOCOL_VERSION, Request, Response, RuntimeHooks};
use std::{net::TcpStream, time::Duration};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};

pub(super) struct Connection(WebSocket<TcpStream>);
impl Connection {
    pub(super) fn accept(stream: TcpStream) -> Result<Self, Error> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(1024 * 1024))
            .max_frame_size(Some(1024 * 1024))
            .max_write_buffer_size(32 * 1024 * 1024);
        let ws = tungstenite::accept_with_config(stream, Some(config)).map_err(|e| {
            Error::Diagnostic(BridgeError::new(
                "ARA-EXP-BRG-002",
                &e.to_string(),
                "connect using WebSocket protocol",
            ))
        })?;
        ws.get_ref().set_nonblocking(true)?;
        Ok(Self(ws))
    }
    pub(super) fn poll(
        &mut self,
        token: &str,
        state: &mut State<impl RuntimeHooks>,
    ) -> Result<bool, Error> {
        match self.0.flush() {
            Ok(()) => {}
            Err(e) if would_block(&e) => return Ok(true),
            Err(e) => return Err(e.into()),
        }
        // One request per client per pass keeps a busy agent from starving the editor.
        let message = match self.0.read() {
            Ok(m) => m,
            Err(e) if would_block(&e) => return Ok(true),
            Err(e) => return Err(e.into()),
        };
        let Message::Text(text) = message else {
            return Ok(!message.is_close());
        };
        let request: Request = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(_) => return Ok(false),
        };
        let authenticated = constant_eq(request.token.as_bytes(), token.as_bytes());
        let result = if !authenticated {
            Err(BridgeError::new(
                "ARA-EXP-BRG-006",
                "authentication failed",
                "use the dev runtime launch token",
            ))
        } else if request.version != PROTOCOL_VERSION {
            Err(BridgeError::new(
                "ARA-EXP-BRG-005",
                "unsupported protocol version",
                "use bridge protocol version 1",
            ))
        } else {
            state.execute(request.command)
        };
        let (result, error) = match result {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        let response = Response {
            version: PROTOCOL_VERSION,
            id: request.id,
            result,
            error,
        };
        let text = serde_json::to_string(&response)?;
        if text.len() > 32 * 1024 * 1024 {
            return Ok(false);
        }
        match self.0.send(Message::text(text)) {
            Ok(()) => {}
            Err(e) if would_block(&e) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(authenticated)
    }
}
fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e,tungstenite::Error::Io(e) if e.kind()==std::io::ErrorKind::WouldBlock)
}
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for (i, byte) in b.iter().enumerate() {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ byte);
    }
    diff == 0
}
