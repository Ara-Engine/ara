//! Normalize resumable socket states before Tungstenite interprets I/O failures.
use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::handshake::{HandshakeError, HandshakeRole};
use tungstenite::{Message, WebSocket};

fn normalize<T>(result: io::Result<T>) -> io::Result<T> {
    result.map_err(|error| {
        // Winsock's WSA_IO_PENDING is a successful pending state, not a lost
        // connection. std's blocking socket owns the OS I/O; keep the WebSocket
        // parser/write buffer alive and let it resume, including automatic pongs.
        // https://learn.microsoft.com/windows/win32/winsock/overlapped-i-o-and-event-objects-2
        let pending = cfg!(windows) && error.raw_os_error() == Some(997);
        if pending {
            thread::sleep(Duration::from_millis(1));
            io::Error::new(io::ErrorKind::WouldBlock, error)
        } else {
            error
        }
    })
}

#[derive(Debug)]
pub(crate) struct Stream<S>(pub(crate) S);

impl<S: Read> Read for Stream<S> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        normalize(self.0.read(bytes))
    }
}

impl<S: Write> Write for Stream<S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        normalize(self.0.write(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        normalize(self.0.flush())
    }
}

/// Resume the same HTTP upgrade state, never resend a partially written upgrade.
pub(crate) fn handshake<R: HandshakeRole>(
    mut state: Result<R::FinalResult, HandshakeError<R>>,
    deadline: Instant,
) -> io::Result<R::FinalResult> {
    loop {
        match state {
            Ok(result) => return Ok(result),
            Err(HandshakeError::Failure(error)) => return Err(io::Error::other(error)),
            Err(HandshakeError::Interrupted(handshake)) => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "WebSocket upgrade deadline exceeded",
                    ));
                }
                thread::sleep(Duration::from_millis(1));
                state = handshake.handshake();
            }
        }
    }
}

/// A WouldBlock write already owns the queued frame. Flush its remainder instead
/// of submitting the message again, which could duplicate mutating RPC requests.
pub(crate) fn send<S: Read + Write>(
    ws: &mut WebSocket<S>,
    message: Message,
    deadline: Instant,
) -> tungstenite::Result<()> {
    match ws.write(message) {
        Ok(()) => {}
        Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
        Err(error) => return Err(error),
    }
    flush(ws, deadline)
}

pub(crate) fn flush<S: Read + Write>(
    ws: &mut WebSocket<S>,
    deadline: Instant,
) -> tungstenite::Result<()> {
    loop {
        match ws.flush() {
            Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "WebSocket output deadline exceeded",
                    )
                    .into());
                }
                thread::sleep(Duration::from_millis(1));
            }
            result => return result,
        }
    }
}

#[cfg(all(test, windows))]
#[path = "transport_tests.rs"]
mod tests;
