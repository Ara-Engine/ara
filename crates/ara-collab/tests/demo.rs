//! The two-process demo: an authority host in a child process and, over WebSocket,
//! two human replicas (optimistic edits, presence at ~300 Hz, a claim), two agents
//! (claim-blocked commands, a branch session) and an observer, all editing one scene
//! at once. Asserts convergence, claim enforcement, the branch merge report, the
//! conflict log and per-participant undo; prints op latency and bandwidth.
//!
//! The child is this test binary re-run as `authority_process` with
//! `ARA_COLLAB_DEMO_ROOT` set.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ara_collab::{OpEvent, ParticipantId, Reject, Replica, Snapshot};
use ara_doc::{AssetId, Edit, RecordId, write};
use ara_host::{Host, HostConfig, Project};
use ara_proto::{ConnId, Endpoint, Incoming, RpcError, jsonrpc};
use ara_rpc::{Client, Handler, ServeOptions};
use serde_json::{Value, json};

const ROOT_ENV: &str = "ARA_COLLAB_DEMO_ROOT";
const EDITS: usize = 100;

/// The authority: the project host behind the WebSocket server, counting bytes per
/// connection (named by the client it began a session as). `demo.stats` answers
/// `{client: [bytes in, bytes out]}`.
struct Authority {
    host: Host,
    bytes: BTreeMap<u64, [u64; 2]>,
    names: BTreeMap<u64, String>,
    /// Time the host spent per method (ms), for the engine-side cost.
    busy: BTreeMap<String, Vec<f64>>,
}

impl Handler for Authority {
    fn message(&mut self, conn: ConnId, text: &str) -> Option<String> {
        self.bytes.entry(conn.0).or_default()[0] += text.len() as u64;
        let reply = match Incoming::parse(text) {
            Ok(Incoming::Request { id, method, .. }) if method == "demo.stats" => {
                let named: BTreeMap<String, [u64; 2]> = self
                    .bytes
                    .iter()
                    .map(|(c, b)| (self.names.get(c).cloned().unwrap_or(c.to_string()), *b))
                    .collect();
                let mut busy = BTreeMap::new();
                for (m, v) in &mut self.busy {
                    busy.insert(m.clone(), [percentile(v, 0.5), percentile(v, 0.95)]);
                }
                Some(jsonrpc::response(id, json!({ "clients": named, "busy": busy })).to_string())
            }
            Ok(Incoming::Request { method, params, .. }) if method == "session.begin" => {
                if let Some(client) = params["client"].as_str() {
                    self.names.insert(conn.0, client.to_owned());
                }
                self.host.handle_text(conn, text)
            }
            Ok(Incoming::Request { method, .. }) => {
                let t = Instant::now();
                let reply = self.host.handle_text(conn, text);
                self.busy.entry(method).or_default().push(ms(t.elapsed()));
                reply
            }
            _ => self.host.handle_text(conn, text),
        };
        if let Some(r) = &reply {
            self.bytes.entry(conn.0).or_default()[1] += r.len() as u64;
        }
        reply
    }

    fn closed(&mut self, conn: ConnId) {
        self.host.closed(conn);
    }

    fn tick(&mut self) {
        self.host.tick();
    }

    fn outbox(&mut self) -> Vec<(ConnId, String)> {
        let out: Vec<(ConnId, String)> = self
            .host
            .drain_outbox()
            .into_iter()
            .map(|(c, v)| (c, v.to_string()))
            .collect();
        for (c, t) in &out {
            self.bytes.entry(c.0).or_default()[1] += t.len() as u64;
        }
        out
    }

    fn stopping(&self) -> bool {
        self.host.stopping()
    }
}

#[test]
#[ignore = "the authority process of `two_process_demo`; it only runs when spawned by it"]
fn authority_process() {
    let Ok(root) = std::env::var(ROOT_ENV) else {
        return;
    };
    let root = PathBuf::from(root);
    let config = HostConfig {
        debounce: Duration::from_secs(3600),
        seed: Some(1),
        ..HostConfig::default()
    };
    let host = Host::open(Project::open(&root).expect("project"), config).expect("host");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let token = ara_rpc::project_token(&root).expect("token");
    println!("AUTHORITY {port} {token}");
    let mut authority = Authority {
        host,
        bytes: BTreeMap::new(),
        names: BTreeMap::new(),
        busy: BTreeMap::new(),
    };
    let opts = ServeOptions {
        listener: Some(listener),
        token,
        stdio: false,
        tick: Duration::from_millis(20),
    };
    ara_rpc::serve(&mut authority, opts).expect("serve");
}

/// Kills the authority if the test fails before shutting it down.
struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn scene() -> String {
    let records: Vec<String> = (0..20)
        .map(|i| {
            format!(
                r#""e_{i:02}": {{ "name": "Thing {i}", "order": "a{i:02}", "Transform": {{ "position": [{i}, 0, 0] }} }}"#
            )
        })
        .collect();
    format!(
        r#"{{ "$schema": "ara://schema/scene/1", "id": "sc_yard0001", "records": {{ {} }} }}"#,
        records.join(", ")
    )
}

fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-collab-demo-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("levels")).expect("mkdir");
    fs::write(dir.join("ara.project.json"), r#"{ "name": "demo" }"#).expect("marker");
    fs::write(dir.join("levels/yard.scene"), scene()).expect("scene");
    dir
}

fn spawn(root: &Path) -> (Kill, u16, String) {
    let exe = std::env::current_exe().expect("test binary");
    let mut child = Command::new(exe)
        .args(["authority_process", "--exact", "--ignored", "--nocapture"])
        .env(ROOT_ENV, root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the authority");
    let stdout = child.stdout.take().expect("stdout");
    let kill = Kill(child);
    for line in BufReader::new(stdout).lines() {
        let line = line.expect("line");
        if let Some(rest) = line.strip_prefix("AUTHORITY ") {
            let (port, token) = rest.split_once(' ').expect("port and token");
            return (kill, port.parse().expect("port"), token.to_owned());
        }
    }
    panic!("the authority exited before listening");
}

fn doc() -> AssetId {
    AssetId::parse("sc_yard0001").expect("id")
}

fn set(id: &str, path: &str, value: Value) -> Edit {
    Edit::Set {
        doc: doc(),
        id: RecordId::parse(id).expect("id"),
        path: path.parse().expect("path"),
        value: Some(value),
    }
}

/// Send times of human submits, by (participant, seq).
type Sent = Arc<Mutex<HashMap<(String, u64), Instant>>>;

/// One connected participant.
struct Peer {
    client: Client,
    session: String,
    replica: Option<Replica>,
    conflicts: usize,
    presence: BTreeMap<String, usize>,
}

impl Peer {
    fn connect(port: u16, token: &str, author: &str, name: &str, root: &Path, join: bool) -> Peer {
        let mut client = Client::connect(port, token).expect("connect");
        let begun = client
            .call("session.begin", json!({ "client": name, "author": author }))
            .expect("begin");
        let session = begun["session"].as_str().expect("session").to_owned();
        let mut peer = Peer {
            client,
            session,
            replica: None,
            conflicts: 0,
            presence: BTreeMap::new(),
        };
        if join {
            let joined = peer
                .call("collab.join", json!({ "name": name, "snapshot": true }))
                .expect("join");
            let snap: Snapshot =
                serde_json::from_value(joined["snapshot"].clone()).expect("snapshot");
            let db = Project::open(root).expect("project").db;
            peer.replica = Some(
                Replica::from_snapshot(ParticipantId(peer.session.clone()), db, &snap)
                    .expect("replica"),
            );
        }
        peer
    }

    fn call(&mut self, method: &str, mut params: Value) -> Result<Value, RpcError> {
        params["session"] = self.session.clone().into();
        self.client.call(method, params)
    }

    /// Applies buffered notifications; returns the op events applied.
    fn pump(&mut self) -> Vec<OpEvent> {
        let mut ops = Vec::new();
        for n in self.client.notifications() {
            match n["method"].as_str() {
                Some("collab.op") => {
                    let ev: OpEvent = serde_json::from_value(n["params"].clone()).expect("op");
                    if let Some(r) = &mut self.replica {
                        r.receive(&ev).expect("in order");
                    }
                    ops.push(ev);
                }
                Some("collab.reject") => {
                    let rj: Reject = serde_json::from_value(n["params"].clone()).expect("reject");
                    if let Some(r) = &mut self.replica {
                        r.rejected(rj.seq);
                    }
                }
                Some("collab.conflict") => self.conflicts += 1,
                Some("collab.presence") => {
                    let who = n["params"]["participant"].as_str().unwrap_or_default();
                    *self.presence.entry(who.to_owned()).or_default() += 1;
                }
                _ => {}
            }
        }
        ops
    }

    /// Edits locally and submits; records the send time.
    fn edit(&mut self, edits: Vec<Edit>, sent: &Sent) -> Duration {
        let r = self.replica.as_mut().expect("replica");
        let s = r
            .edit(edits, None, Some("demo edit".into()))
            .expect("local edit");
        sent.lock()
            .expect("lock")
            .insert((self.session.clone(), s.seq), Instant::now());
        let t = Instant::now();
        let edits: Vec<Value> = s.edits.iter().map(|e| json!(e)).collect();
        // A dropped submit answers with an error and a `collab.reject`; both are fine.
        let _ = self.call(
            "collab.submit",
            json!({ "seq": s.seq, "base": s.base, "edits": edits, "label": "demo edit" }),
        );
        let rtt = t.elapsed();
        self.pump();
        rtt
    }

    /// Catches up (missed ops come from `collab.sync`; duplicates are ignored) until
    /// this replica is at the authority's revision with nothing pending.
    fn sync(&mut self) {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            self.pump();
            let since = self.replica.as_ref().map_or(0, Replica::revision);
            let got = self
                .call("collab.sync", json!({ "since": since }))
                .expect("sync");
            let ops: Vec<OpEvent> = serde_json::from_value(got["ops"].clone()).expect("ops");
            let r = self.replica.as_mut().expect("replica");
            for ev in &ops {
                r.receive(ev).expect("in order");
            }
            self.pump();
            let r = self.replica.as_ref().expect("replica");
            let head = got["revision"].as_u64().expect("revision");
            if r.revision() >= head && r.pending() == 0 {
                return;
            }
            assert!(
                Instant::now() < end,
                "replica stuck at {} of {head}",
                r.revision()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn text(&self) -> String {
        let r = self.replica.as_ref().expect("replica");
        let d = r.workspace().doc("sc_yard0001").expect("doc");
        write(d, r.schema())
    }
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        return 0.0;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
fn two_process_demo() {
    let root = project();
    let (_authority, port, token) = spawn(&root);
    let sent: Sent = Arc::new(Mutex::new(HashMap::new()));
    // `start`: everyone connected (observer, Mia, Sam, agents); `gate`: the phases
    // after it (Mia, Sam, agents).
    let start = Arc::new(Barrier::new(4));
    let gate = Arc::new(Barrier::new(3));
    let started = Instant::now();

    // Observer: timestamps every op it receives (remote apply latency).
    let observer = {
        let (token, root, sent, start) = (token.clone(), root.clone(), sent.clone(), start.clone());
        thread::spawn(move || {
            let mut o = Peer::connect(port, &token, "human", "observer", &root, true);
            start.wait();
            let mut latencies = Vec::new();
            let end = Instant::now() + Duration::from_secs(60);
            let mut idle = 0;
            while Instant::now() < end && idle < 40 {
                match o.client.wait_notification(Duration::from_millis(50)) {
                    Some(n) => {
                        idle = 0;
                        if n["method"] == "collab.op"
                            && let (Some(p), Some(seq)) = (
                                n["params"]["participant"].as_str(),
                                n["params"]["seq"].as_u64(),
                            )
                            && let Some(t0) = sent.lock().expect("lock").get(&(p.to_owned(), seq))
                        {
                            latencies.push(ms(t0.elapsed()));
                        }
                        if n["method"] == "collab.presence" {
                            let who = n["params"]["participant"].as_str().unwrap_or_default();
                            *o.presence.entry(who.to_owned()).or_default() += 1;
                        }
                    }
                    None => idle += 1,
                }
            }
            (latencies, o.presence, o.session)
        })
    };

    // Two humans: optimistic edits on things 0–9, presence on every step, a claim.
    let human = |name: &'static str, salt: usize| {
        let (token, root, sent, gate, start) = (
            token.clone(),
            root.clone(),
            sent.clone(),
            gate.clone(),
            start.clone(),
        );
        thread::spawn(move || {
            let mut p = Peer::connect(port, &token, "human", name, &root, true);
            if name == "Mia" {
                p.call(
                    "collab.claim",
                    json!({ "subjects": ["e_05"], "why": "tuning the gate", "ttlMs": 120000 }),
                )
                .expect("claim");
            }
            start.wait(); // A: everyone ready
            let mut rtts = Vec::new();
            let mut presence_sent = 0;
            for i in 0..EDITS {
                let cam = json!({ "position": [i as f32, 2.0, salt as f32], "rotation": [0.0, i as f32, 0.0] });
                p.call(
                    "collab.presence",
                    json!({ "doc": "levels/yard.scene", "camera": cam, "selection": [format!("e_{:02}", (i + salt) % 10)] }),
                )
                .expect("presence");
                presence_sent += 1;
                // e_05 and e_06 are the decorator's branch subjects; humans leave them
                // alone except for Mia's one rename below.
                const FREE: [usize; 8] = [0, 1, 2, 3, 4, 7, 8, 9];
                let id = format!("e_{:02}", FREE[(i * 7 + salt) % FREE.len()]);
                let edit = match i % 4 {
                    // Both humans rename e_00 on the same steps: same-field conflicts.
                    0 if i % 20 == 0 => set("e_00", "name", json!(format!("{name} {i}"))),
                    0 => set(&id, "name", json!(format!("{name} {i}"))),
                    1 => set(&id, "Transform.position.x", json!(i)),
                    2 => set(&id, "Transform.rotation.y", json!(salt * 10 + i)),
                    _ => set(&id, "Transform.scale", json!([1, 1 + i % 3, 1])),
                };
                rtts.push(ms(p.edit(vec![edit], &sent)));
                if name == "Mia" && i == 50 {
                    rtts.push(ms(
                        p.edit(vec![set("e_05", "name", json!("Gate (Mia)"))], &sent)
                    ));
                }
            }
            gate.wait(); // B: editing done
            p.sync();
            gate.wait(); // C: synced
            if name == "Mia" {
                p.call("collab.release", json!({})).expect("release");
            }
            gate.wait(); // D: claim released; the branch merges on a tick
            thread::sleep(Duration::from_millis(200));
            p.sync();
            // Per-participant undo: Mia names e_01, Sam moves it, Mia undoes.
            if name == "Mia" {
                p.edit(vec![set("e_01", "name", json!("Mia's sign"))], &sent);
                p.sync();
            }
            gate.wait(); // E
            if name == "Sam" {
                p.edit(vec![set("e_01", "Transform.position.x", json!(77))], &sent);
                p.sync();
            }
            gate.wait(); // F
            if name == "Mia" {
                let undone = p.call("collab.undo", json!({})).expect("undo");
                assert!(undone["txn"].is_string(), "{undone}");
            }
            gate.wait(); // G
            thread::sleep(Duration::from_millis(100));
            p.sync();
            (rtts, presence_sent, p.conflicts, p.text())
        })
    };
    let mia = human("Mia", 0);
    let sam = human("Sam", 3);

    // Agents: one hits Mia's claim and works elsewhere; one uses a branch session.
    let agents = {
        let (token, root, gate, start) = (token.clone(), root.clone(), gate.clone(), start.clone());
        thread::spawn(move || {
            let mut builder = Peer::connect(port, &token, "agent:builder", "builder", &root, false);
            let mut decorator =
                Peer::connect(port, &token, "agent:decorator", "decorator", &root, false);
            let opened = decorator
                .call("branch.open", json!({ "why": "Decorate the gate" }))
                .expect("branch");
            let br = opened["id"].as_str().expect("id").to_owned();
            start.wait(); // A
            let blocked = builder
                .call(
                    "cmd.apply",
                    json!({ "doc": "levels/yard.scene", "why": "rename the gate",
                        "commands": [{ "cmd": "property.set", "args": { "entity": "e_05", "path": "name", "value": "Builder gate" } }] }),
                )
                .expect_err("Mia claimed e_05");
            let mut cmd_ms = Vec::new();
            for i in 0..40 {
                let id = format!("e_{}", 15 + i % 5);
                let t = Instant::now();
                builder
                    .call(
                        "cmd.apply",
                        json!({ "doc": "levels/yard.scene", "why": "line up the posts",
                            "commands": [{ "cmd": "transform.set", "args": { "entities": [id], "position": [i, 0, 5] } }] }),
                    )
                    .expect("unclaimed work");
                cmd_ms.push(ms(t.elapsed()));
            }
            decorator
                .call(
                    "cmd.apply",
                    json!({ "doc": "levels/yard.scene", "branch": br,
                        "commands": [
                            { "cmd": "property.set", "args": { "entity": "e_05", "path": "name", "value": "Gate (decorated)" } },
                            { "cmd": "property.set", "args": { "entity": "e_06", "path": "name", "value": "Lantern" } },
                        ] }),
                )
                .expect("branch apply");
            let submitted = decorator
                .call("branch.submit", json!({ "branch": br }))
                .expect("submit");
            gate.wait(); // B
            gate.wait(); // C
            gate.wait(); // D
            let end = Instant::now() + Duration::from_secs(10);
            let status = loop {
                let s = decorator
                    .call("branch.status", json!({ "branch": br }))
                    .expect("status");
                if s["status"] == "merged" || Instant::now() > end {
                    break s;
                }
                thread::sleep(Duration::from_millis(20));
            };
            gate.wait(); // E
            gate.wait(); // F
            gate.wait(); // G
            (blocked, cmd_ms, submitted, status)
        })
    };

    let (latencies, observed_presence, observer_session) = observer.join().expect("observer");
    let (mia_rtt, mia_presence, mia_conflicts, mia_text) = mia.join().expect("Mia");
    let (sam_rtt, _, sam_conflicts, sam_text) = sam.join().expect("Sam");
    let (blocked, mut cmd_ms, submitted, status) = agents.join().expect("agents");
    let elapsed = started.elapsed();

    // The authority's view.
    let mut check = Client::connect(port, &token).expect("connect");
    let begun = check
        .call(
            "session.begin",
            json!({ "client": "check", "author": "human" }),
        )
        .expect("begin");
    let s = begun["session"].as_str().expect("session").to_owned();
    let snap = check
        .call("collab.sync", json!({ "session": s, "snapshot": true }))
        .expect("snapshot");
    let snap: Snapshot = serde_json::from_value(snap["snapshot"].clone()).expect("snapshot");
    let truth = snap.docs[0].text.clone();

    // Convergence.
    assert_eq!(mia_text, truth, "Mia's replica converged");
    assert_eq!(sam_text, truth, "Sam's replica converged");
    // Claim enforcement: the agent was told who and why.
    let d = &blocked.diagnostics()[0];
    assert_eq!(d.code.to_string(), "ARA2500");
    assert!(
        d.message.contains("Mia (human) for \"tuning the gate\""),
        "{}",
        d.message
    );
    // The branch waited for the claim, then merged keeping Mia's live name.
    assert_eq!(submitted["status"], "submitted");
    assert!(
        submitted["blockedBy"][0]
            .as_str()
            .is_some_and(|b| b.contains("Mia"))
    );
    assert_eq!(status["status"], "merged", "{status}");
    assert_eq!(status["report"]["conflicts"][0]["record"], "e_05");
    let live: Value = serde_json::from_str(&truth).expect("scene json");
    assert_eq!(live["records"]["e_05"]["name"], "Gate (Mia)");
    assert_eq!(live["records"]["e_06"]["name"], "Lantern");
    // The conflict log kept the losing same-field edits and told their authors.
    let log = check
        .call("collab.conflicts", json!({ "session": s, "mine": false }))
        .expect("conflicts");
    let overwritten = log["conflicts"]
        .as_array()
        .expect("list")
        .iter()
        .filter(|c| c["kind"] == "overwritten")
        .count();
    assert!(overwritten > 0, "concurrent same-field edits were logged");
    assert!(mia_conflicts + sam_conflicts > 0, "losers were told");
    // Per-participant undo: Mia's rename is gone, Sam's later move stays.
    assert_eq!(live["records"]["e_01"]["Transform"]["position"][0], 77);
    assert_ne!(live["records"]["e_01"]["name"], "Mia's sign");
    // Presence was rate limited: Mia sent one per step, the observer got ≤ 20 Hz.
    let received = observed_presence
        .iter()
        .filter(|(k, _)| **k != observer_session)
        .map(|(_, v)| *v)
        .max()
        .unwrap_or(0);
    let budget = (elapsed.as_millis() as usize) / 50 + 4;

    // Numbers.
    let stats = check.call("demo.stats", json!({})).expect("stats");
    let mut rtt: Vec<f64> = mia_rtt.iter().chain(&sam_rtt).copied().collect();
    let mut remote = latencies.clone();
    println!(
        "collab demo over {:.1} s ({} human edits, 40 agent commands, {} presence updates per human):",
        elapsed.as_secs_f64(),
        2 * EDITS,
        mia_presence
    );
    println!(
        "  submit round trip  p50 {:.2} ms  p95 {:.2} ms",
        percentile(&mut rtt, 0.5),
        percentile(&mut rtt, 0.95)
    );
    println!(
        "  op to other client p50 {:.2} ms  p95 {:.2} ms ({} ops)",
        percentile(&mut remote, 0.5),
        percentile(&mut remote, 0.95),
        latencies.len()
    );
    println!(
        "  agent cmd.apply    p50 {:.2} ms  p95 {:.2} ms",
        percentile(&mut cmd_ms, 0.5),
        percentile(&mut cmd_ms, 0.95)
    );
    println!(
        "  presence: {mia_presence} sent by Mia, {received} broadcasts reached the observer (budget {budget})"
    );
    println!(
        "  conflicts logged: {overwritten} overwritten; {} to Mia, {} to Sam",
        mia_conflicts, sam_conflicts
    );
    for (method, [p50, p95]) in [
        ("collab.submit", &stats["busy"]["collab.submit"]),
        ("cmd.apply", &stats["busy"]["cmd.apply"]),
        ("collab.presence", &stats["busy"]["collab.presence"]),
    ]
    .map(|(m, v)| {
        (
            m,
            [v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0)],
        )
    }) {
        println!("  authority time in {method}: p50 {p50:.3} ms  p95 {p95:.3} ms");
    }
    for (client, [inb, out]) in stats["clients"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), [v[0].as_u64()?, v[1].as_u64()?])))
    {
        println!(
            "  {client}: in {:.1} kB, out {:.1} kB ({:.1} / {:.1} kB/s)",
            inb as f64 / 1024.0,
            out as f64 / 1024.0,
            inb as f64 / 1024.0 / elapsed.as_secs_f64(),
            out as f64 / 1024.0 / elapsed.as_secs_f64()
        );
    }
    assert!(
        received <= budget,
        "presence was not rate limited: {received} > {budget}"
    );
    // The authority exits right after answering; the reply may not make it out.
    let _ = check.call("daemon.shutdown", json!({ "session": s }));
}
