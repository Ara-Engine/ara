use super::*;
use std::io::Cursor;
use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::derive_accept_key;
use tungstenite::protocol::Role;

struct Faults {
    incoming: Cursor<Vec<u8>>,
    written: Vec<u8>,
    write_pending: bool,
    read_pending: bool,
    permanent: Option<i32>,
}
impl Faults {
    fn new(incoming: Vec<u8>) -> Self {
        Self {
            incoming: Cursor::new(incoming),
            written: Vec::new(),
            write_pending: false,
            read_pending: false,
            permanent: None,
        }
    }
}
impl Read for Faults {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(error) = self.permanent {
            return Err(io::Error::from_raw_os_error(error));
        }
        self.read_pending = !self.read_pending;
        if !self.read_pending {
            return Err(io::Error::from_raw_os_error(997));
        }
        let size = bytes.len().min(64);
        self.incoming.read(&mut bytes[..size])
    }
}
impl Write for Faults {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(error) = self.permanent {
            return Err(io::Error::from_raw_os_error(error));
        }
        self.write_pending = !self.write_pending;
        if !self.write_pending {
            return Err(io::Error::from_raw_os_error(997));
        }
        let size = bytes.len().min(7);
        self.written.extend_from_slice(&bytes[..size]);
        Ok(size)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn pending_partial_writes_flush_one_request_without_replaying_it() {
    let mut client =
        WebSocket::from_raw_socket(Stream(Faults::new(Vec::new())), Role::Client, None);
    let request = "one mutating RPC request".repeat(8);
    send(
        &mut client,
        Message::text(request.clone()),
        Instant::now() + Duration::from_secs(5),
    )
    .expect("resume accepted frame");
    let bytes = client.get_ref().0.written.clone();
    let mut server = WebSocket::from_raw_socket(Cursor::new(bytes), Role::Server, None);
    assert_eq!(
        server.read().expect("complete frame"),
        Message::text(request)
    );
    assert!(
        matches!(
            server.read(),
            Err(tungstenite::Error::Protocol(
                tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
            ))
        ),
        "must not contain a second request or a corrupted trailing frame"
    );
}

#[test]
fn pending_automatic_pong_is_retained_while_reading_the_response() {
    let mut server = WebSocket::from_raw_socket(Cursor::new(Vec::new()), Role::Server, None);
    server
        .send(Message::Ping(vec![1, 2, 3].into()))
        .expect("ping");
    server.send(Message::text("response")).expect("response");
    let mut client = WebSocket::from_raw_socket(
        Stream(Faults::new(server.get_ref().get_ref().clone())),
        Role::Client,
        None,
    );
    let mut received = false;
    for _ in 0..128 {
        match client.read() {
            Ok(Message::Text(text)) => {
                assert_eq!(text, "response");
                received = true;
                break;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
            other => panic!("pending control-frame write must not lose connection: {other:?}"),
        }
    }
    assert!(received);
    flush(&mut client, Instant::now() + Duration::from_secs(5)).expect("pong output");
    let mut pong = WebSocket::from_raw_socket(
        Cursor::new(client.get_ref().0.written.clone()),
        Role::Server,
        None,
    );
    assert_eq!(
        pong.read().expect("one pong"),
        Message::Pong(vec![1, 2, 3].into())
    );
}

#[test]
fn pending_upgrade_resumes_partial_headers_without_another_get() {
    let request = "ws://127.0.0.1/".into_client_request().expect("request");
    let accept = derive_accept_key(request.headers()["Sec-WebSocket-Key"].as_bytes());
    let reply = format!(
        "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    let state = tungstenite::client(request, Stream(Faults::new(reply.into_bytes())));
    let (ws, _) =
        handshake(state, Instant::now() + Duration::from_secs(5)).expect("upgrade resumes");
    let written = String::from_utf8(ws.get_ref().0.written.clone()).expect("HTTP request");
    assert!(written.starts_with("GET / HTTP/1.1\r\n"));
    assert_eq!(written.matches("GET / HTTP/1.1").count(), 1);
    assert!(written.ends_with("\r\n\r\n"));
}

#[test]
fn reset_is_fatal_and_permanently_pending_output_has_a_deadline() {
    for code in [10054, 995] {
        let mut reset = Stream(Faults {
            permanent: Some(code),
            ..Faults::new(Vec::new())
        });
        let error = reset
            .read(&mut [0; 1])
            .expect_err("reset/cancellation remains fatal");
        assert_eq!(error.raw_os_error(), Some(code));
        assert_ne!(error.kind(), io::ErrorKind::WouldBlock);
    }
    let mut ws = WebSocket::from_raw_socket(
        Stream(Faults {
            permanent: Some(997),
            ..Faults::new(Vec::new())
        }),
        Role::Client,
        None,
    );
    let started = Instant::now();
    assert!(
        matches!(send(&mut ws, Message::text("mutate"), started + Duration::from_millis(30)), Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}
