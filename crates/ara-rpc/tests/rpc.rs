//! The WebSocket and HTTP transport against an echo handler: calls, notifications,
//! and token checks.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use ara_proto::{ConnId, Endpoint, Incoming, jsonrpc};
use ara_rpc::http::{HttpRequest, HttpResponse};
use ara_rpc::{Client, Handler, ServeOptions, serve};
use serde_json::{Value, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// Echoes requests; `ping-me` also queues a notification for the caller.
#[derive(Default)]
struct Echo {
    outbox: Vec<(ConnId, String)>,
}

impl Handler for Echo {
    fn message(&mut self, conn: ConnId, text: &str) -> Option<String> {
        let Ok(Incoming::Request { id, method, params }) = Incoming::parse(text) else {
            return None;
        };
        if method == "ping-me" {
            let note = jsonrpc::notification("ops", json!({ "n": 1 }));
            self.outbox.push((conn, note.to_string()));
        }
        Some(jsonrpc::response(id, json!({ "method": method, "params": params })).to_string())
    }

    fn http(&mut self, req: &HttpRequest) -> HttpResponse {
        HttpResponse::json(
            200,
            &json!({ "path": req.path, "body": String::from_utf8_lossy(&req.body) }),
        )
    }

    fn outbox(&mut self) -> Vec<(ConnId, String)> {
        std::mem::take(&mut self.outbox)
    }
}

fn start() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        let opts = ServeOptions {
            listener: Some(listener),
            token: TOKEN.into(),
            stdio: false,
            tick: Duration::from_millis(50),
        };
        serve(&mut Echo::default(), opts)
    });
    port
}

fn post(port: u16, headers: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(
        s,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .expect("write");
    let mut out = String::new();
    s.read_to_string(&mut out).expect("read");
    out
}

#[test]
fn websocket_calls_round_trip_and_notifications_arrive() {
    let port = start();
    let mut c = Client::connect(port, TOKEN).expect("connects");
    let r = c.call("query", json!({ "q": "Light" })).expect("answer");
    assert_eq!(r, json!({ "method": "query", "params": { "q": "Light" } }));
    c.call("ping-me", json!({})).expect("answer");
    let note = c
        .wait_notification(Duration::from_secs(5))
        .expect("a notification");
    assert_eq!(note, json!({ "method": "ops", "params": { "n": 1 } }));
}

#[test]
fn connections_need_the_token_and_a_local_origin() {
    let port = start();
    assert!(Client::connect(port, "wrong").is_err());

    let refused = post(port, "", "{}");
    assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");
    assert!(refused.contains("ARA1807"));
    let foreign = post(
        port,
        &format!("Authorization: Bearer {TOKEN}\r\nOrigin: https://evil.example\r\n"),
        "{}",
    );
    assert!(foreign.starts_with("HTTP/1.1 403"), "{foreign}");

    let ok = post(
        port,
        &format!("Authorization: Bearer {TOKEN}\r\n"),
        r#"{"a":1}"#,
    );
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    let body: Value =
        serde_json::from_str(ok.split("\r\n\r\n").nth(1).expect("body")).expect("json");
    assert_eq!(body, json!({ "path": "/mcp", "body": r#"{"a":1}"# }));
}
