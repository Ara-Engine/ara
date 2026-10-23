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
fn agents_apply_through_advisory_claims_and_optional_branches_merge_immediately() {
    let dir = project("claims");
    let mut h = open(&dir, 1);
    let mia = begin(&mut h, "human");
    let bot = begin(&mut h, "agent:bot");
    call_on(
        &mut h,
        7,
        "collab.join",
        json!({"session":mia,"name":"Mia"}),
    );
    call(
        &mut h,
        "collab.claim",
        json!({"session":mia,"subjects":["e_flr1"]}),
    );
    call(&mut h, "cmd.apply", rename(&bot, "e_flr1", "Ground", None));
    assert_eq!(name_of(&mut h, &mia, "e_flr1"), "Ground");
    let branch = call(
        &mut h,
        "branch.open",
        json!({"session":bot,"why":"optional isolation"}),
    )["id"]
        .as_str()
        .expect("branch")
        .to_owned();
    call(
        &mut h,
        "cmd.apply",
        rename(&bot, "e_flr1", "New ground", Some(&branch)),
    );
    assert_eq!(name_of(&mut h, &mia, "e_flr1"), "Ground");
    let submitted = call(
        &mut h,
        "branch.submit",
        json!({"session":bot,"branch":branch}),
    );
    assert_eq!(submitted["status"], "merged");
    assert_eq!(name_of(&mut h, &mia, "e_flr1"), "New ground");
}

#[test]
fn gestures_hold_only_the_active_field_then_release_agent_ops_in_order() {
    let dir = project("gestures");
    let mut h = open(&dir, 11);
    let human = begin(&mut h, "human");
    let bot = begin(&mut h, "agent:bot");
    let gesture = |active| json!({"session":human,"doc":"levels/arena.scene","entity":"e_flr1","path":"name","active":active});
    call(&mut h, "editor.gesture", gesture(true));
    let mut mixed = rename(&bot, "e_flr1", "Agent first", None);
    mixed["commands"].as_array_mut().expect("commands").push(json!({"cmd":"property.set","args":{"entity":"e_flr1","path":"Transform.position.x","value":7}}));
    call(&mut h, "cmd.apply", mixed);
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Floor");
    let doc = h.store().workspace().doc("sc_arena001").expect("scene");
    assert_eq!(
        doc.record("e_flr1").expect("floor")["Transform"]["position"][0],
        7
    );
    call(
        &mut h,
        "cmd.apply",
        rename(&bot, "e_flr1", "Agent second", None),
    );
    call(
        &mut h,
        "cmd.apply",
        rename(&human, "e_flr1", "Human finish", None),
    );
    call(&mut h, "editor.gesture", gesture(false));
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Agent second");
    let names: Vec<_> = h
        .store()
        .history()
        .iter()
        .filter(|t| t.author == ara_ops::Author::Agent("bot".into()))
        .flat_map(|t| t.ops.iter())
        .filter_map(|op| match op {
            ara_doc::Op::Set { path, value, .. } if path.to_string() == "name" => {
                value.as_ref().and_then(Value::as_str)
            }
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["Agent first", "Agent second"]);
}

#[test]
fn an_abandoned_gesture_cannot_hold_an_agent_for_more_than_two_seconds() {
    let dir = project("gesture-timeout");
    let mut h = open(&dir, 12);
    let human = begin(&mut h, "human");
    let bot = begin(&mut h, "agent:bot");
    call(
        &mut h,
        "editor.gesture",
        json!({"session":human,"doc":"levels/arena.scene","entity":"e_flr1","path":"name","active":true}),
    );
    call(
        &mut h,
        "cmd.apply",
        rename(&bot, "e_flr1", "Timed out", None),
    );
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Floor");
    std::thread::sleep(Duration::from_millis(2050));
    h.tick();
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Timed out");
}

#[test]
fn a_gesture_split_command_is_still_one_undoable_agent_turn() {
    let dir = project("gesture-turn");
    let mut h = open(&dir, 14);
    let human = begin(&mut h, "human");
    let bot = begin(&mut h, "agent:bot");
    let initial_transform = h
        .store()
        .workspace()
        .doc("sc_arena001")
        .expect("doc")
        .record("e_flr1")
        .expect("floor")["Transform"]
        .clone();
    let gesture = |active| json!({"session":human,"doc":"levels/arena.scene","entity":"e_flr1","path":"name","active":active});
    call(&mut h, "editor.gesture", gesture(true));
    let mut mixed = rename(&bot, "e_flr1", "Agent floor", None);
    mixed["commands"].as_array_mut().expect("commands").push(json!({"cmd":"property.set","args":{"entity":"e_flr1","path":"Transform.position.x","value":7}}));
    let result = call(&mut h, "cmd.apply", mixed);
    call(
        &mut h,
        "cmd.apply",
        rename(&human, "e_flr1", "Human finish", None),
    );
    call(&mut h, "editor.gesture", gesture(false));
    call(
        &mut h,
        "review.revert",
        json!({"session":human,"txn":result["txn"]}),
    );
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Human finish");
    let transform = &h
        .store()
        .workspace()
        .doc("sc_arena001")
        .expect("doc")
        .record("e_flr1")
        .expect("floor")["Transform"];
    assert_eq!(transform["position"], json!([0, 0, 0]));
    assert_eq!(transform["scale"], initial_transform["scale"]);
}

#[test]
fn replica_agent_submits_and_optional_branches_obey_gesture_safety() {
    let dir = project("gesture-apis");
    let mut h = open(&dir, 15);
    let human = begin(&mut h, "human");
    let bot = begin(&mut h, "agent:bot");
    let gesture = |active| json!({"session":human,"doc":"levels/arena.scene","entity":"e_flr1","path":"name","active":active});
    call(&mut h, "editor.gesture", gesture(true));
    let submit = json!({"session":bot,"seq":1,"base":0,"why":"Replica turn","edits":[{"op":"set","doc":"sc_arena001","id":"e_flr1","path":"name","value":"Replica floor"}]});
    let ack = call(&mut h, "collab.submit", submit.clone());
    assert_eq!(ack["gestureHeld"], 1);
    assert_eq!(call(&mut h, "collab.submit", submit)["replayed"], true);
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Floor");
    call(&mut h, "editor.gesture", gesture(false));
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Replica floor");
    let br = call(
        &mut h,
        "branch.open",
        json!({"session":bot,"why":"optional branch"}),
    )["id"]
        .as_str()
        .expect("branch")
        .to_owned();
    call(
        &mut h,
        "cmd.apply",
        rename(&bot, "e_flr1", "Branch floor", Some(&br)),
    );
    call(&mut h, "editor.gesture", gesture(true));
    assert_eq!(
        call(&mut h, "branch.submit", json!({"session":bot,"branch":br}))["status"],
        "submitted"
    );
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Replica floor");
    call(&mut h, "editor.gesture", gesture(false));
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Branch floor");
}

#[test]
fn ordinary_command_overwrites_notice_both_participants_and_diagnostics() {
    let dir = project("overwrite");
    let mut h = open(&dir, 13);
    let human = begin(&mut h, "human");
    let bot = begin(&mut h, "agent:bot");
    call_on(
        &mut h,
        7,
        "collab.join",
        json!({"session":human,"name":"Mia"}),
    );
    call_on(
        &mut h,
        8,
        "collab.join",
        json!({"session":bot,"name":"Builder"}),
    );
    call(
        &mut h,
        "cmd.apply",
        rename(&human, "e_flr1", "Human floor", None),
    );
    h.drain_outbox();
    call(
        &mut h,
        "cmd.apply",
        rename(&bot, "e_flr1", "Agent floor", None),
    );
    let events = h.drain_outbox();
    for conn in [ConnId(7), ConnId(8)] {
        assert!(
            events
                .iter()
                .any(|(c, n)| *c == conn && n["method"] == "collab.conflict")
        );
        assert!(
            events
                .iter()
                .any(|(c, n)| *c == conn && n["method"] == "editor.overwrite")
        );
    }
    let logs = call(
        &mut h,
        "logs",
        json!({"session":bot,"codes":["ARA2503"],"limit":10}),
    );
    assert!(
        logs["entries"].as_array().is_some_and(|e| !e.is_empty()),
        "{logs}"
    );
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Agent floor");
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
