//! The serve loop. One thread owns the [`Handler`]; connection threads only move
//! text. WebSocket threads poll their socket with a short read timeout so they can
//! also deliver notifications queued for them.

use std::collections::BTreeMap;
use std::io::{self, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use ara_diag::{Diagnostic, codes};
use ara_proto::{ConnId, RpcError, jsonrpc, lines};
use tungstenite::{Message, WebSocket};

use crate::http::{self, HttpRequest, HttpResponse};

/// The connection ID of the stdio channel.
pub const STDIO: ConnId = ConnId(1);
const POLL: Duration = Duration::from_millis(20);

/// The engine side of a transport.
pub trait Handler {
    /// One text message (a WebSocket frame or a stdio line) from `conn`; returns
    /// the reply text, if any.
    fn message(&mut self, conn: ConnId, text: &str) -> Option<String>;
    /// A plain HTTP request (already authenticated). Default: 404.
    fn http(&mut self, req: &HttpRequest) -> HttpResponse {
        let _ = req;
        HttpResponse::empty(404)
    }
    /// A connection closed.
    fn closed(&mut self, conn: ConnId) {
        let _ = conn;
    }
    /// Periodic work while idle (and at least every tick interval when busy).
    fn tick(&mut self) {}
    /// Notifications queued for connections.
    fn outbox(&mut self) -> Vec<(ConnId, String)> {
        Vec::new()
    }
    /// True when the loop should stop.
    fn stopping(&self) -> bool {
        false
    }
}

/// What to serve.
pub struct ServeOptions {
    /// A bound `127.0.0.1` listener for WebSocket and HTTP connections.
    pub listener: Option<TcpListener>,
    /// The token connections must present.
    pub token: String,
    /// Also read messages from stdin and write replies to stdout ([`STDIO`]); the
    /// loop stops when stdin closes.
    pub stdio: bool,
    /// Tick interval.
    pub tick: Duration,
}

enum Event {
    Opened(ConnId, Sender<Outgoing>),
    Message(ConnId, String),
    Closed(ConnId),
    Http(HttpRequest, Sender<HttpResponse>),
    StdinClosed,
}

enum Outgoing {
    Text(String),
    Flushed(Sender<()>),
}

/// Runs until the handler stops, stdin closes (with `stdio`) or every event
/// source is gone.
pub fn serve(handler: &mut dyn Handler, opts: ServeOptions) -> io::Result<()> {
    let (tx, rx) = channel();
    if let Some(listener) = opts.listener {
        let tx = tx.clone();
        let token = opts.token.clone();
        thread::spawn(move || accept(listener, token, tx));
    }
    if opts.stdio {
        let tx = tx.clone();
        thread::spawn(move || read_stdin(tx));
    }
    drop(tx);
    run(handler, rx, opts.tick)
}

fn run(handler: &mut dyn Handler, rx: Receiver<Event>, tick: Duration) -> io::Result<()> {
    let mut conns: BTreeMap<ConnId, Sender<Outgoing>> = BTreeMap::new();
    let mut stdout = io::stdout();
    let mut last_tick = Instant::now();
    loop {
        let mut send = |conn: ConnId, text: String, conns: &BTreeMap<ConnId, Sender<Outgoing>>| {
            if conn == STDIO {
                let _ = writeln!(stdout, "{text}").and_then(|()| stdout.flush());
            } else if let Some(c) = conns.get(&conn) {
                let _ = c.send(Outgoing::Text(text));
            }
        };
        match rx.recv_timeout(tick) {
            Ok(Event::Opened(id, out)) => {
                conns.insert(id, out);
            }
            Ok(Event::Message(id, text)) => {
                if let Some(reply) = handler.message(id, &text) {
                    send(id, reply, &conns);
                }
            }
            Ok(Event::Closed(id)) => {
                conns.remove(&id);
                handler.closed(id);
            }
            Ok(Event::Http(req, reply)) => {
                let _ = reply.send(handler.http(&req));
            }
            Ok(Event::StdinClosed) | Err(RecvTimeoutError::Disconnected) => {
                handler.tick();
                return Ok(());
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if last_tick.elapsed() >= tick {
            handler.tick();
            last_tick = Instant::now();
        }
        for (conn, text) in handler.outbox() {
            send(conn, text, &conns);
        }
        if handler.stopping() {
            // A daemon must deliver its shutdown reply before its process exits.
            // FIFO barriers acknowledge the socket writers, with one bounded deadline.
            let (ack, received) = channel();
            let mut waiting = 0;
            for connection in conns.values() {
                if connection.send(Outgoing::Flushed(ack.clone())).is_ok() {
                    waiting += 1;
                }
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            for _ in 0..waiting {
                if received
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .is_err()
                {
                    break;
                }
            }
            return Ok(());
        }
    }
}

fn read_stdin(tx: Sender<Event>) {
    let mut reader = BufReader::new(io::stdin());
    while let Ok(Some(line)) = lines::read_line(&mut reader) {
        if tx.send(Event::Message(STDIO, line)).is_err() {
            return;
        }
    }
    let _ = tx.send(Event::StdinClosed);
}

fn accept(listener: TcpListener, token: String, tx: Sender<Event>) {
    // 0 is the in-process caller and 1 is stdio.
    for (next, stream) in (2..).zip(listener.incoming().flatten()) {
        let (tx, token, id) = (tx.clone(), token.clone(), ConnId(next));
        thread::spawn(move || {
            let _ = connection(stream, &token, id, &tx);
        });
    }
}

/// Why a request is refused, if it is: a foreign browser origin (403) or a missing
/// or wrong token (401).
fn refuse(req: &HttpRequest, token: &str) -> Option<(u16, String)> {
    if let Some(origin) = req.header("origin") {
        let host = origin
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let host = host.split(':').next().unwrap_or(host);
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Some((403, format!("origin `{origin}` is not allowed")));
        }
    }
    let presented = req
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| req.query_param("token"));
    if presented != Some(token) {
        return Some((
            401,
            "present the project token from `.ara/token` (`Authorization: Bearer <token>`)".into(),
        ));
    }
    None
}

fn refusal_body(status: u16, why: &str) -> serde_json::Value {
    let code = if status == 401 {
        codes::UNAUTHORIZED
    } else {
        codes::INVALID_REQUEST
    };
    jsonrpc::error_response(
        serde_json::Value::Null,
        &RpcError::from(Diagnostic::error(code, why)),
    )
}

fn connection(
    mut stream: TcpStream,
    token: &str,
    id: ConnId,
    tx: &Sender<Event>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let Some(head) = http::peek_head(&stream)? else {
        return Ok(());
    };
    let upgrade = head
        .header("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if !upgrade {
        let req = http::read_request(&mut stream)?;
        let resp = match refuse(&req, token) {
            Some((status, why)) => HttpResponse::json(status, &refusal_body(status, &why)),
            None => {
                let (reply_tx, reply_rx) = channel();
                if tx.send(Event::Http(req, reply_tx)).is_err() {
                    return Ok(());
                }
                reply_rx.recv().unwrap_or_else(|_| HttpResponse::empty(503))
            }
        };
        return http::write_response(&mut stream, &resp);
    }
    if let Some((status, why)) = refuse(&head, token) {
        let resp = HttpResponse::json(status, &refusal_body(status, &why));
        return http::write_response(&mut stream, &resp);
    }
    // Already authenticated from the peeked head; the handshake re-reads it.
    let mut ws = tungstenite::accept(stream).map_err(io::Error::other)?;
    ws.get_mut().set_read_timeout(Some(POLL))?;
    let (out_tx, out_rx) = channel();
    if tx.send(Event::Opened(id, out_tx)).is_err() {
        return Ok(());
    }
    let result = pump(&mut ws, id, tx, &out_rx);
    let _ = tx.send(Event::Closed(id));
    result
}

/// Moves frames in both directions until either side closes.
fn pump(
    ws: &mut WebSocket<TcpStream>,
    id: ConnId,
    tx: &Sender<Event>,
    out: &Receiver<Outgoing>,
) -> io::Result<()> {
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                if tx.send(Event::Message(id, text.to_string())).is_err() {
                    return Ok(());
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(e) => return Err(io::Error::other(e)),
        }
        while let Ok(message) = out.try_recv() {
            match message {
                Outgoing::Text(text) => ws.send(Message::text(text)).map_err(io::Error::other)?,
                Outgoing::Flushed(ack) => {
                    ws.flush().map_err(io::Error::other)?;
                    let _ = ack.send(());
                }
            }
        }
    }
}
