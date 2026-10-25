//! Shared loopback fake GitHub. Fixture keys are never used as release trust roots.
use crate::{Asset, Channel, FeedConfig, Kind, Manifest, canonical_manifest, sign};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

/// A fake response, optionally interrupted to exercise resumable streaming.
#[derive(Clone, Default)]
pub struct Response {
    /// HTTP status, normally 200.
    pub status: u16,
    /// Full response payload.
    pub body: Vec<u8>,
    /// Additional response headers.
    pub headers: Vec<(String, String)>,
    /// Close a full GET early; Range retries deliver the remainder.
    pub interrupt: Option<usize>,
    /// Stall the response body after headers to exercise in-flight cancellation.
    pub delay_ms: u64,
}
impl Response {
    /// Successful response with this body.
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            ..Self::default()
        }
    }
}
/// Recorded fake-server request.
#[derive(Clone, Debug)]
pub struct Request {
    /// Path and query, including page cursor.
    pub path: String,
    /// Lowercased header names.
    pub headers: BTreeMap<String, String>,
}
/// Local-only fixture HTTP server; no request can reach GitHub.
pub struct Server {
    /// HTTP loopback origin bound to an ephemeral port.
    pub base: String,
    routes: Arc<Mutex<BTreeMap<String, Response>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
impl Server {
    /// Start an isolated fake server.
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = Arc::new(Mutex::new(BTreeMap::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (r, q, flag) = (routes.clone(), requests.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                if let Ok((socket, _)) = listener.accept() {
                    respond(socket, &r, &q);
                } else {
                    thread::sleep(Duration::from_millis(2));
                }
            }
        });
        Self {
            base,
            routes,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    /// Replace a route; tests can change rate limits, asset bytes or ETags mid-run.
    pub fn set(&self, path: &str, response: Response) {
        self.routes.lock().unwrap().insert(path.into(), response);
    }
    /// Requests observed so far.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
    /// Trust only the synthetic fixture key and allow only this loopback origin.
    pub fn config(&self) -> FeedConfig {
        FeedConfig::for_test(self.base.clone(), vec![test_public_key()])
    }
    /// Add a signed release and archive routes; return its GitHub API listing entry.
    pub fn release(&self, kind: Kind, version: &str, channel: Channel, archive: Vec<u8>) -> Value {
        self.release_build(kind, version, channel, archive, None)
    }
    /// Publish a signed same-version source build through the local fixture feed.
    pub fn release_build(
        &self,
        kind: Kind,
        version: &str,
        channel: Channel,
        archive: Vec<u8>,
        build: Option<crate::BuildIdentity>,
    ) -> Value {
        let tag = format!("{}-v{version}", kind.name());
        let name = format!("ara-{}-{version}-windows-x64.zip", kind.name());
        let prefix = format!("/assets/{tag}");
        let manifest = Manifest {
            build,
            format: "ara.release/1".into(),
            kind,
            version: version.into(),
            channel,
            min_hub_version: "0.0.1-alpha".into(),
            notes: "Local test release".into(),
            platform: "windows-x64".into(),
            assets: vec![Asset {
                name: name.clone(),
                size: archive.len() as u64,
                sha256: hex::encode(Sha256::digest(&archive)),
                url: format!("{}{prefix}/{name}", self.base),
            }],
        };
        let bytes = canonical_manifest(&manifest).unwrap();
        let (signature, _) = sign(&bytes, &test_seed()).unwrap();
        self.set(&format!("{prefix}/release.json"), Response::ok(bytes));
        self.set(
            &format!("{prefix}/release.json.sig"),
            Response::ok(signature),
        );
        self.set(&format!("{prefix}/{name}"), Response::ok(archive));
        json!({"tag_name":tag,"prerelease":channel == Channel::Preview,"draft":false,"assets":[{"name":"release.json","browser_download_url":format!("{}{prefix}/release.json",self.base)},{"name":"release.json.sig","browser_download_url":format!("{}{prefix}/release.json.sig",self.base)}]})
    }
    /// Replace the first releases page.
    pub fn releases(&self, releases: Vec<Value>) {
        self.set(
            "/repos/Snowy7/ara/releases?per_page=100",
            Response::ok(serde_json::to_vec(&releases).unwrap()),
        );
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
fn respond(
    mut socket: TcpStream,
    routes: &Mutex<BTreeMap<String, Response>>,
    requests: &Mutex<Vec<Request>>,
) {
    // Accepted sockets can inherit the listener's nonblocking mode on Windows.
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = match socket.read(&mut chunk) {
            Ok(n) => n,
            Err(error) => {
                eprintln!("fake GitHub request read failed: {error}");
                return;
            }
        };
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    let path = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.to_lowercase(), v.trim().to_owned()))
        })
        .collect();
    requests.lock().unwrap().push(Request {
        path: path.clone(),
        headers: headers.clone(),
    });
    let mut response = routes
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or(Response {
            status: 404,
            ..Response::default()
        });
    let mut offset = 0;
    if let Some(etag) = headers.get("if-none-match")
        && response
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("etag") && v == etag)
    {
        response.status = 304;
        response.body.clear();
    } else if let Some(range) = headers.get("range") {
        offset = range
            .strip_prefix("bytes=")
            .unwrap()
            .trim_end_matches('-')
            .parse()
            .unwrap();
        response.status = 206;
        response.headers.push((
            "Content-Range".into(),
            format!(
                "bytes {offset}-{}/{}",
                response.body.len() - 1,
                response.body.len()
            ),
        ));
    }
    let length = response.body.len() - offset;
    let mut head = format!(
        "HTTP/1.1 {} Fake\r\nContent-Length: {length}\r\nConnection: close\r\n",
        response.status
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = socket.write_all(head.as_bytes());
    if response.delay_ms > 0 {
        thread::sleep(Duration::from_millis(response.delay_ms));
    }
    let end = if offset == 0 {
        response.interrupt.unwrap_or(response.body.len())
    } else {
        response.body.len()
    };
    let _ = socket.write_all(&response.body[offset..end]);
}
/// Ephemeral fixture seed generated once per test process; never stored in source.
pub fn test_seed() -> String {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed).expect("fixture randomness");
        hex::encode(seed)
    })
    .clone()
}
/// Fixture public key, never included in production trust roots.
pub fn test_public_key() -> String {
    sign(b"test", &test_seed()).unwrap().1
}
/// Scratch data always stays under target, never tracked files.
pub fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/update-tests")
        .join(format!(
            "{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}
/// Small engine archive with enough layout to test service installation boundaries.
pub fn engine_zip(version: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for name in [
        "ara.exe",
        "ara-editor.exe",
        "ara-player.exe",
        "templates/blank/README.txt",
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"dummy fixture, never executed").unwrap();
    }
    zip.start_file("engine.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(
        serde_json::to_string(&json!({"format":"ara.engine/1","version":version}))
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}
