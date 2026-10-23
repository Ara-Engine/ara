//! Collaboration through the protocol: claim-aware `cmd.apply` for agents, the
//! "who" summary, branch sessions through `cmd.apply {branch}`, notifications to
//! joined connections, and claims and branches surviving a restart.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ara_host::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::{Value, json};

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for entry in fs::read_dir(from).expect("read fixture dir").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-host-collab-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project"),
        &dir,
    );
    dir
}

fn open(dir: &Path, seed: u64) -> Host {
    let config = HostConfig {
        debounce: Duration::from_secs(3600),
        watch_every: Duration::from_secs(3600),
        scan_every: Duration::from_secs(3600),
        seed: Some(seed),
        ..HostConfig::default()
    };
    Host::open(Project::open(dir).expect("project"), config).expect("host")
}

fn call_on(h: &mut Host, conn: u64, method: &str, params: Value) -> Value {
    h.call(ConnId(conn), method, params)
        .unwrap_or_else(|e| panic!("{method} failed:\n{e}"))
}

fn call(h: &mut Host, method: &str, params: Value) -> Value {
    call_on(h, 0, method, params)
}

fn begin(h: &mut Host, author: &str) -> String {
    let r = call(
        h,
        "session.begin",
        json!({ "client": "test", "author": author }),
    );
    r["session"].as_str().expect("handle").to_owned()
}

fn rename(session: &str, entity: &str, name: &str, branch: Option<&str>) -> Value {
    let mut p = json!({
        "session": session,
        "doc": "levels/arena.scene",
        "why": "rename",
        "commands": [{ "cmd": "property.set", "args": { "entity": entity, "path": "name", "value": name } }],
    });
    if let Some(b) = branch {
        p["branch"] = b.into();
    }
    p
}

fn name_of(h: &mut Host, s: &str, entity: &str) -> Value {
    let r = call(
        h,
        "inspect",
        json!({ "session": s, "doc": "levels/arena.scene", "entities": [entity], "format": "json" }),
    );
    r["entities"][0]["name"].clone()
}

#[test]
fn agents_are_blocked_by_claims_and_work_in_branches_instead() {
    let dir = project("claims");
    let mut h = open(&dir, 1);
    let mia = begin(&mut h, "human");
    let bot = begin(&mut h, "agent:bot");
    // Mia's editor joins on connection 7 and claims the floor.
    let joined = call_on(
        &mut h,
        7,
        "collab.join",
        json!({ "session": mia, "name": "Mia" }),
    );
    assert_eq!(joined["participant"]["name"], "Mia");
    let claimed = call(
        &mut h,
        "collab.claim",
        json!({ "session": mia, "subjects": ["name:Floor"], "why": "resizing the floor", "ttlMs": 600000 }),
    );
    assert!(
        claimed["text"]
            .as_str()
            .is_some_and(|t| t.contains("levels/arena.scene#e_flr1 (Floor)"))
    );

    // The agent never joined; its edit on the floor is rejected, naming Mia.
    let err = h
        .call(
            ConnId::LOCAL,
            "cmd.apply",
            rename(&bot, "e_flr1", "Ground", None),
        )
        .expect_err("claimed");
    let d = &err.diagnostics()[0];
    assert_eq!(d.code.to_string(), "ARA2500");
    assert!(
        d.message.contains("Mia (human) for \"resizing the floor\""),
        "{}",
        d.message
    );
    // A dry run tells it the same before it tries.
    let mut dry = rename(&bot, "e_flr1", "Ground", None);
    dry["dryRun"] = true.into();
    assert!(h.call(ConnId::LOCAL, "cmd.apply", dry).is_err());
    // The summary shows it what to avoid.
    let who = call(&mut h, "collab.who", json!({ "session": bot }));
    let text = who["text"].as_str().expect("text");
    assert!(text.contains("- Mia (human)"), "{text}");
    assert!(text.contains("- bot (agent, you)"), "{text}");
    assert!(
        text.contains("levels/arena.scene#e_flr1 (Floor): Mia \"resizing the floor\""),
        "{text}"
    );
    // Agents cannot claim over a human.
    let err = h
        .call(
            ConnId::LOCAL,
            "collab.claim",
            json!({ "session": bot, "subjects": ["e_flr1"] }),
        )
        .expect_err("held");
    assert_eq!(err.diagnostics()[0].code.to_string(), "ARA2501");
    // The agent does the work in a branch session instead.
    let opened = call(
        &mut h,
        "branch.open",
        json!({ "session": bot, "why": "rename the floor" }),
    );
    let br = opened["id"].as_str().expect("branch id").to_owned();
    // (Forked before Mia's next edit, so both rename the floor.)
    // Mia herself is never blocked, and her edit reaches her connection as an op.
    call(
        &mut h,
        "cmd.apply",
        rename(&mia, "e_flr1", "Big floor", None),
    );
    let notes = h.drain_outbox();
    let op = notes
        .iter()
        .find(|(c, n)| *c == ConnId(7) && n["method"] == "collab.op")
        .expect("Mia's connection gets the op");
    assert_eq!(op.1["params"]["participant"], mia.as_str());
    let applied = call(
        &mut h,
        "cmd.apply",
        rename(&bot, "e_flr1", "Ground", Some(&br)),
    );
    assert!(
        applied["text"]
            .as_str()
            .is_some_and(|t| t.contains("not live"))
    );
    assert_eq!(
        name_of(&mut h, &mia, "e_flr1"),
        "Big floor",
        "live untouched"
    );
    let submitted = call(
        &mut h,
        "branch.submit",
        json!({ "session": bot, "branch": br }),
    );
    assert_eq!(submitted["status"], "submitted");
    assert!(
        submitted["text"]
            .as_str()
            .is_some_and(|t| t.contains("waiting on"))
    );

    // Restart: the claim (Mia's session is live) and the branch come back.
    h.flush();
    drop(h);
    let mut h = open(&dir, 2);
    let who = call(&mut h, "collab.who", json!({ "session": bot }));
    assert!(
        who["text"]
            .as_str()
            .is_some_and(|t| t.contains("e_flr1 (Floor): Mia"))
    );
    let status = call(
        &mut h,
        "branch.status",
        json!({ "session": bot, "branch": br }),
    );
    assert_eq!(status["status"], "submitted");

    // Mia releases; the branch merges on the next tick, keeping Mia's live name
    // (both renamed the floor) and reporting the conflict.
    call(&mut h, "collab.release", json!({ "session": mia }));
    h.tick();
    let status = call(
        &mut h,
        "branch.status",
        json!({ "session": bot, "branch": br }),
    );
    assert_eq!(status["status"], "merged", "{status}");
    assert_eq!(status["report"]["conflicts"][0]["record"], "e_flr1");
    assert_eq!(name_of(&mut h, &mia, "e_flr1"), "Big floor");
    let conflicts = call(&mut h, "collab.conflicts", json!({ "session": bot }));
    assert_eq!(conflicts["conflicts"][0]["kind"], "merge");
}

#[test]
fn replicas_submit_and_undo_per_participant() {
    let dir = project("submit");
    let mut h = open(&dir, 3);
    let a = begin(&mut h, "human");
    let b = begin(&mut h, "human");
    let joined = call_on(
        &mut h,
        5,
        "collab.join",
        json!({ "session": a, "name": "A", "snapshot": true }),
    );
    assert_eq!(joined["snapshot"]["revision"], 0);
    call_on(
        &mut h,
        6,
        "collab.join",
        json!({ "session": b, "name": "B" }),
    );
    let set = |s: &str, seq: u64, path: &str, v: Value| {
        json!({ "session": s, "seq": seq, "base": 0, "edits": [
            { "op": "set", "doc": "sc_arena001", "id": "e_flr1", "path": path, "value": v }
        ] })
    };
    call(
        &mut h,
        "collab.submit",
        set(&a, 1, "name", json!("A floor")),
    );
    let ack = call(
        &mut h,
        "collab.submit",
        set(&b, 1, "Transform.position", json!([0, 1, 0])),
    );
    assert_eq!(ack["overwrote"], 0, "different fields merge");
    let ack = call(
        &mut h,
        "collab.submit",
        set(&b, 2, "name", json!("B floor")),
    );
    assert_eq!(
        ack["overwrote"], 1,
        "same field: B wins, A's change is logged"
    );
    let again = call(
        &mut h,
        "collab.submit",
        set(&b, 2, "name", json!("B floor")),
    );
    assert_eq!(again["replayed"], true);
    let notes = h.drain_outbox();
    assert!(
        notes
            .iter()
            .any(|(c, n)| *c == ConnId(5) && n["method"] == "collab.conflict"),
        "A hears about the overwrite"
    );
    // B undoes their own latest change only: the name goes back to A's.
    let undone = call(&mut h, "collab.undo", json!({ "session": b }));
    assert!(undone["txn"].is_string());
    assert_eq!(name_of(&mut h, &a, "e_flr1"), "A floor");
    let replay = call(&mut h, "collab.sync", json!({ "session": a, "since": 0 }));
    assert_eq!(replay["ops"].as_array().map(Vec::len), Some(4));
    assert_eq!(replay["ops"][2]["seq"], 2);
}
