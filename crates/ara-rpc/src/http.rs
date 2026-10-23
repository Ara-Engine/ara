//! Just enough HTTP/1.1 for the daemon's non-WebSocket endpoints (MCP Streamable
//! HTTP at `/mcp`): one request per connection, `Content-Length` bodies, then close.

use std::io::{self, Read, Write};
use std::net::TcpStream;

use serde_json::Value;

/// Largest request body accepted.
pub const MAX_BODY: usize = 16 << 20;
const MAX_HEAD: usize = 64 << 10;

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// `GET`, `POST`, ...
    pub method: String,
    /// Path without the query (`/mcp`).
    pub path: String,
    /// The query string without `?`.
    pub query: String,
    /// Headers in order, names as sent.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// A header value, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// A query parameter (no percent-decoding; tokens are hex).
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }
}

/// A response to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Extra headers (`Content-Length` and `Connection` are added).
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A JSON body.
    pub fn json(status: u16, body: &Value) -> Self {
        HttpResponse {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    }

    /// No body.
    pub fn empty(status: u16) -> Self {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    }
}

/// Parses a request head (request line and headers, without the blank line).
pub fn parse_head(head: &str) -> Option<HttpRequest> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(HttpRequest {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        body: Vec::new(),
    })
}

/// The request head waiting on `stream`, without consuming it (so a WebSocket
/// handshake can still read it).
pub fn peek_head(stream: &TcpStream) -> io::Result<Option<HttpRequest>> {
    let mut buf = vec![0u8; 8192];
    for _ in 0..200 {
        let n = stream.peek(&mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        if let Some(end) = find(&buf[..n], b"\r\n\r\n") {
            return Ok(parse_head(&String::from_utf8_lossy(&buf[..end])));
        }
        if n == buf.len() {
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(None)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Reads one whole request (head and `Content-Length` body).
pub fn read_request(stream: &mut TcpStream) -> io::Result<HttpRequest> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(end) = find(&data, b"\r\n\r\n") {
            break end;
        }
        if data.len() > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed",
            ));
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let mut req = parse_head(&String::from_utf8_lossy(&data[..end]))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad request line"))?;
    let len: usize = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    let mut body = data[end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    req.body = body;
    Ok(req)
}

/// Writes a response and closes the exchange (`Connection: close`).
pub fn write_response(stream: &mut impl Write, resp: &HttpResponse) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (k, v) in &resp.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        resp.body.len()
    ));
    stream.write_all(head.as_bytes())?;
    stream.write_all(&resp.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_parse_with_case_insensitive_headers() {
        let req = parse_head(
            "POST /mcp?token=ab12 HTTP/1.1\r\nHost: x\r\nmcp-protocol-version: 2026-07-28",
        )
        .expect("parses");
        assert_eq!((req.method.as_str(), req.path.as_str()), ("POST", "/mcp"));
        assert_eq!(req.query_param("token"), Some("ab12"));
        assert_eq!(req.header("MCP-Protocol-Version"), Some("2026-07-28"));
        let mut out = Vec::new();
        write_response(&mut out, &HttpResponse::empty(202)).expect("writes");
        assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 202 Accepted\r\n"));
    }
}
