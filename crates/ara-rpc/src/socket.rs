//! Bounded synchronous I/O without starting blocking Winsock receive operations.
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct Socket {
    stream: TcpStream,
    #[cfg(windows)]
    read_timeout: Duration,
    #[cfg(windows)]
    write_timeout: Duration,
}

impl Socket {
    pub(crate) fn new(stream: TcpStream, timeout: Duration) -> io::Result<Self> {
        #[cfg(windows)]
        stream.set_nonblocking(true)?;
        #[cfg(not(windows))]
        {
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
        }
        Ok(Self {
            stream,
            #[cfg(windows)]
            read_timeout: timeout,
            #[cfg(windows)]
            write_timeout: timeout,
        })
    }

    pub(crate) fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        #[cfg(windows)]
        {
            self.read_timeout = timeout;
            Ok(())
        }
        #[cfg(not(windows))]
        self.stream.set_read_timeout(Some(timeout))
    }

    pub(crate) fn peek(&self, bytes: &mut [u8]) -> io::Result<usize> {
        #[cfg(windows)]
        self.ready(false, self.read_timeout)?;
        self.stream.peek(bytes)
    }

    #[cfg(windows)]
    fn ready(&self, write: bool, timeout: Duration) -> io::Result<()> {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            POLLRDNORM, POLLWRNORM, WSAGetLastError, WSAPOLLFD, WSAPoll,
        };
        let mut fd = WSAPOLLFD {
            fd: self.stream.as_raw_socket() as _,
            events: if write { POLLWRNORM } else { POLLRDNORM },
            revents: 0,
        };
        // SAFETY: One initialized descriptor refers to our live owned socket;
        // WSAPoll only borrows this array for the bounded synchronous wait.
        let result = unsafe {
            WSAPoll(
                &mut fd,
                1,
                timeout.as_millis().clamp(1, i32::MAX as u128) as i32,
            )
        };
        match result {
            // SAFETY: Read this thread's Winsock error immediately after failure.
            -1 => Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() })),
            0 => Err(io::ErrorKind::TimedOut.into()),
            _ => Ok(()), // recv/send reports EOF, reset or an invalid socket.
        }
    }
}

impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        #[cfg(windows)]
        self.ready(false, self.read_timeout)?;
        self.stream.read(bytes)
    }
}

impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        #[cfg(windows)]
        self.ready(true, self.write_timeout)?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::mpsc, thread};

    #[test]
    fn timed_out_reads_never_consume_a_later_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let (send, receive) = mpsc::channel();
        let peer = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            for byte in receive {
                socket.write_all(&[byte]).expect("reply");
            }
        });
        let mut socket = Socket::new(
            TcpStream::connect(address).expect("connect"),
            Duration::from_millis(2),
        )
        .expect("nonblocking socket");
        for byte in 0..32 {
            let mut reply = [0];
            assert_eq!(
                socket
                    .stream
                    .read(&mut reply)
                    .expect_err("nonblocking")
                    .kind(),
                io::ErrorKind::WouldBlock,
            );
            assert_eq!(
                socket
                    .read(&mut reply)
                    .expect_err("readiness timeout")
                    .kind(),
                io::ErrorKind::TimedOut,
            );
            send.send(byte).expect("send reply after timeout");
            socket
                .set_read_timeout(Duration::from_secs(2))
                .expect("timeout");
            socket.read_exact(&mut reply).expect("late reply survives");
            assert_eq!(reply, [byte]);
            socket
                .set_read_timeout(Duration::from_millis(2))
                .expect("timeout");
        }
        drop(send);
        peer.join().expect("peer");
    }
}
