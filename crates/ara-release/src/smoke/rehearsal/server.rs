//! File-streaming GitHub Releases mirror; signed URLs/bytes remain unchanged.
use super::*;
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Server {
    pub api: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn start(artifacts: &Path, log: &Path) -> Result<Self> {
        let mut routes = BTreeMap::new();
        let mut releases = Vec::new();
        for kind in ["hub", "engine"] {
            let tag = format!("{kind}-v{}", env!("CARGO_PKG_VERSION"));
            let directory = artifacts.join("published").join(&tag);
            let bytes = fs::read(directory.join("release.json"))?;
            let signature = fs::read(directory.join("release.json.sig"))?;
            let release = ara_update::verify(&bytes, &signature, &ara_update::trusted_keys())?;
            let mut assets = Vec::new();
            for path in support::files(&directory)? {
                let name = path
                    .file_name()
                    .context("release asset name")?
                    .to_str()
                    .context("UTF-8 asset")?;
                ensure!(
                    fs::metadata(&path)?.len() < 2 * 1024 * 1024 * 1024,
                    "GitHub release asset must be under 2 GiB: {name}"
                );
                let route = format!("/Ara-Engine/ara/releases/download/{tag}/{name}");
                assets.push(json!({"name":name,"browser_download_url":format!("https://github.com{route}")}));
                routes.insert(route, path);
            }
            for asset in &release.manifest().assets {
                ensure!(
                    ara_update::sha256_file(&directory.join(&asset.name))? == asset.sha256
                        && fs::metadata(directory.join(&asset.name))?.len() == asset.size,
                    "dry-run asset differs from its signed manifest: {}",
                    asset.name
                );
            }
            releases.push(json!({"tag_name":tag,"prerelease":true,"draft":false,"assets":assets}));
        }
        let body = serde_json::to_vec(&releases)?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let api = format!("http://{}", listener.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let mut log = fs::File::create(log)?;
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        if let Err(error) = respond(socket, &routes, &body, &mut log) {
                            let _ = writeln!(log, "ERROR {error:#}");
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            api,
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn respond(
    mut socket: TcpStream,
    routes: &BTreeMap<String, PathBuf>,
    body: &[u8],
    log: &mut fs::File,
) -> Result<()> {
    // Accepted sockets inherit the listener's nonblocking mode on Windows.
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut request = BufReader::new(socket.try_clone()?);
    let mut line = String::new();
    request.read_line(&mut line)?;
    let path = line
        .split_whitespace()
        .nth(1)
        .context("HTTP request path")?
        .split('?')
        .next()
        .context("path")?
        .to_owned();
    let mut offset = None;
    let mut etag = false;
    loop {
        line.clear();
        if request.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        ensure!(
            !lower.starts_with("authorization:"),
            "rehearsal must never receive a GitHub token"
        );
        if lower.starts_with("range: bytes=") {
            offset = lower
                .trim()
                .strip_prefix("range: bytes=")
                .and_then(|s| s.strip_suffix('-'))
                .and_then(|s| s.parse::<u64>().ok());
        }
        etag |= lower.starts_with("if-none-match:") && lower.contains("ara-rehearsal");
    }
    writeln!(log, "GET {path} range={offset:?} etag={etag}")?;
    log.flush()?;
    if path == "/repos/Ara-Engine/ara/releases" {
        if etag {
            write!(
                socket,
                "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )?;
        } else {
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nETag: \"ara-rehearsal\"\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )?;
            socket.write_all(body)?;
        }
    } else if let Some(path) = routes.get(&path) {
        let mut file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        let start = offset.unwrap_or(0);
        ensure!(start < size, "range outside asset");
        let status = if offset.is_some() {
            "206 Partial Content"
        } else {
            "200 OK"
        };
        write!(
            socket,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n",
            size - start
        )?;
        if offset.is_some() {
            write!(
                socket,
                "Content-Range: bytes {start}-{}/{size}\r\n",
                size - 1
            )?;
        }
        write!(socket, "Connection: close\r\n\r\n")?;
        file.seek(SeekFrom::Start(start))?;
        std::io::copy(&mut file.take(size - start), &mut socket)?;
    } else {
        write!(
            socket,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_assets_stream_completely_from_an_inherited_nonblocking_socket() {
        let target =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release-mirror-tests");
        fs::create_dir_all(&target).expect("target");
        let root = tempfile::tempdir_in(target).expect("isolated mirror");
        let payload = vec![0x5a; 4 * 1024 * 1024];
        let asset = root.path().join("asset.zip");
        fs::write(&asset, &payload).expect("large asset");
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback");
        let address = listener.local_addr().expect("address");
        let reader = thread::spawn(move || {
            let mut client = TcpStream::connect(address).expect("connect");
            client
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("bounded read");
            client
                .write_all(b"GET /asset.zip HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .expect("request");
            let mut response = Vec::new();
            let mut chunk = [0; 16 * 1024];
            loop {
                let size = client.read(&mut chunk).expect("response");
                if size == 0 {
                    break;
                }
                response.extend_from_slice(&chunk[..size]);
                thread::sleep(Duration::from_millis(1));
            }
            response
        });
        let (socket, _) = listener.accept().expect("accept");
        // Windows inherits this from the nonblocking listener. Set it explicitly
        // so the regression also exercises the same backpressure on Unix.
        socket.set_nonblocking(true).expect("inherited mode");
        let mut log = fs::File::create(root.path().join("requests.log")).expect("log");
        let result = respond(
            socket,
            &BTreeMap::from([("/asset.zip".into(), asset)]),
            b"[]",
            &mut log,
        );
        let response = reader.join().expect("reader");
        result.expect("large streaming response must handle backpressure");
        let start = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP headers")
            + 4;
        assert_eq!(&response[start..], payload);
    }
}
