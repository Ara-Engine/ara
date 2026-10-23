//! The host through its protocol surface: round trips, scopes, dry runs, symbols,
//! review, rollback, crash recovery, external edits and service registration.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ara_diag::{Diagnostic, FixAction, codes};
use ara_host::{Cx, Host, HostConfig, Method, Project, Service};
use ara_proto::{ConnId, Scope};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project")
}

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

/// A fresh copy of the fixture project for one test.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-host-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&fixtures(), &dir);
    dir
}

/// A host that never flushes on its own (only on `flush`, drop or `doc.sync`) and
/// checks files on every tick.
fn open(dir: &Path, seed: u64) -> Host {
    let config = HostConfig {
        debounce: Duration::from_secs(3600),
        watch_every: Duration::ZERO,
        scan_every: Duration::ZERO,
        seed: Some(seed),
        ..HostConfig::default()
    };
    Host::open(Project::open(dir).expect("project"), config).expect("host")
}

fn call(h: &mut Host, method: &str, params: Value) -> Value {
    h.call(ConnId::LOCAL, method, params)
        .unwrap_or_else(|e| panic!("{method} failed:\n{e}"))
}

fn fail(h: &mut Host, method: &str, params: Value) -> Diagnostic {
    h.call(ConnId::LOCAL, method, params)
        .expect_err(method)
        .diagnostics()
        .remove(0)
}

fn begin(h: &mut Host, author: &str) -> String {
    let r = call(
        h,
        "session.begin",
        json!({ "client": "test", "author": author }),
    );
    r["session"].as_str().expect("handle").to_owned()
}

fn scene(dir: &Path) -> String {
    fs::read_to_string(dir.join("levels/arena.scene")).expect("scene file")
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
fn json_rpc_round_trips_and_errors_carry_diagnostics() {
    let dir = project("rpc");
    let mut h = open(&dir, 1);
    let reply = |h: &mut Host, text: &str| -> Value {
        serde_json::from_str(&h.handle_text(ConnId::LOCAL, text).expect("a response"))
            .expect("JSON")
    };
    let r = reply(
        &mut h,
        r#"{"jsonrpc":"2.0","id":1,"method":"rpc.discover"}"#,
    );
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["protocol"], "ara-ep/1");
    assert!(
        r["result"]["methods"]
            .as_array()
            .expect("methods")
            .iter()
            .any(|m| m["name"] == "cmd.apply" && m["params"]["properties"]["commands"].is_object())
    );

    let r = reply(
        &mut h,
        r#"{"jsonrpc":"2.0","id":"a","method":"sesion.begin","params":{}}"#,
    );
    assert_eq!(r["error"]["code"], -32601);
    let d = &r["error"]["data"]["diagnostics"][0];
    assert_eq!(d["code"], "ARA1802");
    assert!(
        d["message"]
            .as_str()
            .is_some_and(|m| m.contains("`session.begin`"))
    );

    let r = reply(
        &mut h,
        r#"{"jsonrpc":"2.0","id":2,"method":"play.start","params":{}}"#,
    );
    assert_eq!(r["error"]["data"]["diagnostics"][0]["code"], "ARA1806");

    let d = fail(
        &mut h,
        "query",
        json!({ "session": "ses_nope", "q": "Light" }),
    );
    assert_eq!(d.code, codes::SESSION_NOT_FOUND);
    assert!(
        matches!(&d.fixes[0].action, FixAction::Call { method, .. } if method == "session.begin")
    );

    let s = begin(&mut h, "human");
    let d = fail(&mut h, "query", json!({ "session": s, "qq": "Light" }));
    assert_eq!(
        (d.code, d.location.path.as_deref()),
        (codes::UNKNOWN_FIELD, Some("params.qq"))
    );
    assert_eq!(
        h.handle_text(
            ConnId::LOCAL,
            r#"{"jsonrpc":"2.0","method":"rpc.discover"}"#
        ),
        None,
        "notifications get no response"
    );
}

#[test]
fn scopes_are_granted_by_policy_and_enforced_before_running() {
    let dir = project("scopes");
    let mut h = open(&dir, 2);
    let r = call(
        &mut h,
        "session.begin",
        json!({ "client": "claude-code", "scopes": ["read", "net"] }),
    );
    assert_eq!(r["author"], "agent:claude-code");
    assert_eq!(
        (r["granted"].clone(), r["denied"].clone()),
        (json!(["read"]), json!(["net"]))
    );
    let s = r["session"].as_str().expect("handle").to_owned();

    let d = fail(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "commands": [{ "cmd": "entity.create" }] }),
    );
    assert_eq!(
        (d.code, d.location.command),
        (codes::MISSING_SCOPE, Some(0)),
        "the registry checks the command's scope"
    );
    let d = fail(&mut h, "history.undo", json!({ "session": s }));
    assert_eq!(
        d.code,
        codes::MISSING_SCOPE,
        "the host checks the method's scope"
    );
    assert!(d.message.contains("`edit:scene`"), "{}", d.message);

    let d = fail(
        &mut h,
        "session.begin",
        json!({ "client": "x", "scopes": ["edit:scen"] }),
    );
    assert_eq!(d.code, codes::UNKNOWN_SCOPE);

    let agent = call(&mut h, "session.begin", json!({ "client": "codex" }));
    let granted: Vec<Scope> = serde_json::from_value(agent["granted"].clone()).expect("scopes");
    assert_eq!(granted, Scope::agent_default());
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1805"] }),
    );
    assert_eq!(
        logs["entries"].as_array().map(Vec::len),
        Some(1),
        "the denial is recorded"
    );
}

#[test]
fn dry_run_equals_apply_and_symbols_map_to_real_ids() {
    let dir = project("dryrun");
    let mut h = open(&dir, 3);
    let s = begin(&mut h, "agent:claude");
    let batch = |dry: bool| {
        json!({
            "session": s, "doc": "levels/arena.scene", "why": "Add a crate on a new pad", "dryRun": dry,
            "commands": [
                { "cmd": "entity.create", "as": "$pad", "args": { "name": "Pad", "components": { "Transform": {} } } },
                { "cmd": "prefab.instantiate", "as": "$crate", "args": { "prefab": "@prefabs/crate.prefab", "parent": "$pad", "position": [0, 0.5, 0] } },
                { "cmd": "transform.set", "args": { "entities": ["$pad"], "position": [2, 0, 3] } }
            ]
        })
    };
    let dry = call(&mut h, "cmd.apply", batch(true));
    assert_eq!(
        (dry["dryRun"].clone(), dry["revision"].clone()),
        (json!(true), json!(0))
    );
    let real = call(&mut h, "cmd.apply", batch(false));
    for key in ["ids", "diff", "ops", "outputs"] {
        assert_eq!(
            dry[key], real[key],
            "{key} differs between dry run and apply"
        );
    }
    let pad = real["ids"]["$pad"]
        .as_str()
        .expect("$pad mapped")
        .to_owned();
    assert!(pad.starts_with("e_"), "{pad}");
    assert_eq!(real["diff"]["created"].as_array().map(Vec::len), Some(2));
    assert!(
        real["checkpoint"]
            .as_str()
            .is_some_and(|c| c.starts_with("cp_")),
        "agent turns checkpoint"
    );
    assert_eq!(name_of(&mut h, &s, &pad), "Pad");

    let d = fail(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "commands": [{ "cmd": "transform.set", "as": "$x", "args": { "entities": ["e_flr1"] } }] }),
    );
    assert_eq!(d.location.path.as_deref(), Some("commands[0].as"));
    let d = fail(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "commands": [{ "cmd": "transform.set", "args": { "entities": ["e_flr1"], "position": [1, 2] } }] }),
    );
    assert_eq!(
        d.location.path.as_deref(),
        Some("commands[0].args.position")
    );
}

#[test]
fn review_accepts_and_reverts_per_transaction_and_entity() {
    let dir = project("review");
    let mut h = open(&dir, 4);
    let agent = begin(&mut h, "agent:claude");
    let human = begin(&mut h, "human");
    let r = call(
        &mut h,
        "cmd.apply",
        json!({ "session": agent, "doc": "levels/arena.scene", "why": "Rename and add a marker",
                "commands": [
                    { "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Ground" } },
                    { "cmd": "entity.create", "as": "$m", "args": { "name": "Marker" } }
                ] }),
    );
    let txn = r["txn"].as_str().expect("txn").to_owned();
    let marker = r["ids"]["$m"].as_str().expect("marker").to_owned();
    let pending = call(&mut h, "review.list", json!({ "session": human }));
    assert_eq!(pending["items"][0]["txn"], txn.as_str());

    let d = fail(
        &mut h,
        "review.accept",
        json!({ "session": agent, "txn": txn }),
    );
    assert_eq!(
        d.code,
        codes::NOT_REVIEWABLE,
        "agents cannot accept their own changes"
    );

    call(
        &mut h,
        "review.revert",
        json!({ "session": human, "txn": txn, "entity": "e_flr1" }),
    );
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Floor");
    assert_eq!(
        name_of(&mut h, &human, &marker),
        "Marker",
        "the other entity stays"
    );
    let history = call(
        &mut h,
        "history.list",
        json!({ "session": human, "limit": 5 }),
    );
    let original = history["items"]
        .as_array()
        .and_then(|items| items.iter().find(|i| i["txn"] == txn.as_str()))
        .expect("listed")
        .clone();
    assert_eq!(
        original["status"], "applied",
        "the marker is still unreviewed"
    );

    call(
        &mut h,
        "review.accept",
        json!({ "session": human, "txn": txn }),
    );
    let accepted = call(
        &mut h,
        "review.list",
        json!({ "session": human, "status": "accepted" }),
    );
    assert_eq!(
        accepted["items"][0]["review"][format!("sc_arena001/{marker}")],
        "accepted"
    );
    assert_eq!(
        accepted["items"][0]["review"]["sc_arena001/e_flr1"],
        "reverted"
    );
    let d = fail(
        &mut h,
        "review.accept",
        json!({ "session": human, "all": true }),
    );
    assert_eq!(d.code, codes::NOT_REVIEWABLE, "nothing left");
}

#[test]
fn rollback_returns_to_a_turn_checkpoint_all_or_nothing() {
    let dir = project("rollback");
    let original = scene(&dir);
    let mut h = open(&dir, 5);
    let s = begin(&mut h, "agent:claude");
    let mut checkpoints = Vec::new();
    for name in ["One", "Two"] {
        let r = call(
            &mut h,
            "cmd.apply",
            json!({ "session": s, "doc": "sc_arena001", "why": format!("rename to {name}"),
                    "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": name } }] }),
        );
        checkpoints.push(r["checkpoint"].as_str().expect("checkpoint").to_owned());
    }
    let diff = call(
        &mut h,
        "session.diff",
        json!({ "session": s, "checkpoint": checkpoints[1] }),
    );
    assert_eq!(diff["text"], "~ sc_arena001/e_flr1 name: One → Two\n");
    let r = call(
        &mut h,
        "session.rollback",
        json!({ "session": s, "to": checkpoints[0] }),
    );
    assert_eq!(r["reverted"].as_array().map(Vec::len), Some(2));
    h.flush();
    assert_eq!(scene(&dir), original, "back to the file as it was");
    let d = fail(
        &mut h,
        "session.rollback",
        json!({ "session": s, "to": "cp_nope" }),
    );
    assert_eq!(d.code, codes::CHECKPOINT_NOT_FOUND);
}

#[test]
fn the_journal_replays_commits_lost_in_a_crash() {
    let dir = project("crash");
    let original = scene(&dir);
    let mut h = open(&dir, 6);
    let s = begin(&mut h, "agent:claude");
    let r = call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "why": "rename the floor",
                "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Ground" } }] }),
    );
    let txn = r["txn"].as_str().expect("txn").to_owned();
    h.kill();
    assert_eq!(scene(&dir), original, "the crash lost the unflushed write");

    let mut h = open(&dir, 60);
    assert!(
        scene(&dir).contains("\"Ground\""),
        "recovered and flushed on open"
    );
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1906"] }),
    );
    assert_eq!(logs["entries"].as_array().map(Vec::len), Some(1));
    let history = call(&mut h, "history.list", json!({ "session": s }));
    assert_eq!(
        history["items"][0]["txn"],
        txn.as_str(),
        "same transaction ID"
    );
    assert_eq!(
        history["items"][0]["status"], "applied",
        "review state survives"
    );
    call(&mut h, "history.undo", json!({ "session": s }));
    drop(h);
    assert_eq!(scene(&dir), original, "undo works across the restart");

    let h = open(&dir, 61);
    assert_eq!(h.store().revision(), 2, "nothing replays twice");
}

#[test]
fn external_edits_merge_in_or_are_reported_without_overwriting() {
    let dir = project("external");
    let mut h = open(&dir, 7);
    let s = begin(&mut h, "agent:claude");

    fs::write(
        dir.join("levels/arena.scene"),
        scene(&dir).replace("\"Floor\"", "\"Ground floor\""),
    )
    .expect("edit");
    h.tick();
    assert_eq!(name_of(&mut h, &s, "e_flr1"), "Ground floor");
    let history = call(&mut h, "history.list", json!({ "session": s, "limit": 1 }));
    assert_eq!(history["items"][0]["label"], "external edit");
    assert_eq!(history["items"][0]["author"], "human");

    // An unsaved agent change, then a conflicting edit on disk.
    call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene",
                "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Agent" } }] }),
    );
    let human_text = scene(&dir).replace("\"Ground floor\"", "\"Human\"");
    fs::write(dir.join("levels/arena.scene"), &human_text).expect("edit");
    h.tick();
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1908"] }),
    );
    assert_eq!(logs["entries"].as_array().map(Vec::len), Some(1), "{logs}");
    assert_eq!(name_of(&mut h, &s, "e_flr1"), "Agent", "memory kept");
    h.flush();
    assert_eq!(scene(&dir), human_text, "the file is not overwritten");
    let status = call(&mut h, "session.status", json!({ "session": s }));
    assert_eq!(
        status["files"]["diverged"]["levels/arena.scene"],
        "conflict"
    );

    call(
        &mut h,
        "doc.sync",
        json!({ "session": s, "doc": "levels/arena.scene", "keep": "memory" }),
    );
    assert!(scene(&dir).contains("\"Agent\""));

    fs::write(dir.join("levels/arena.scene"), "{ not json").expect("break");
    h.tick();
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1909"] }),
    );
    assert_eq!(logs["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(name_of(&mut h, &s, "e_flr1"), "Agent");
}

#[test]
fn query_and_inspect_speak_scene_to_text() {
    let dir = project("text");
    let mut h = open(&dir, 8);
    let s = begin(&mut h, "human");
    let summary = call(
        &mut h,
        "inspect",
        json!({ "session": s, "doc": "levels/arena.scene", "depth": 9 }),
    );
    insta::assert_snapshot!("inspect_summary", summary["text"].as_str().expect("text"));
    let details = call(
        &mut h,
        "inspect",
        json!({ "session": s, "doc": "levels/arena.scene", "entities": ["e_lmp1", "e_stk2/e_c002"] }),
    );
    insta::assert_snapshot!("inspect_details", details["text"].as_str().expect("text"));

    let q = call(
        &mut h,
        "query",
        json!({ "session": s, "q": "MeshRenderer under:e_stk2", "fields": ["MeshRenderer.tint"], "limit": 2 }),
    );
    insta::assert_snapshot!("query_page", q["text"].as_str().expect("text"));
    assert_eq!(q["total"], 3);
    let next = call(
        &mut h,
        "query",
        json!({ "session": s, "q": "MeshRenderer under:e_stk2", "cursor": q["next"], "detail": "handles" }),
    );
    // `e_c002/e_lid1` is removed by the instance, so the last match is the crate.
    assert_eq!(
        next["items"],
        json!([{ "entity": "e_stk2/e_c002", "doc": "sc_arena001" }])
    );
    let lights = call(
        &mut h,
        "query",
        json!({ "session": s, "filter": { "where": [{ "path": "Light.intensity", "op": "gt", "value": 500 }] } }),
    );
    assert_eq!(lights["items"][0]["entity"], "e_lmp1");
}

/// Params of `play.step` in the test service.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StepParams {
    /// Session handle.
    #[allow(dead_code)] // Checked by the host before the call.
    session: String,
    /// Ticks to advance.
    ticks: u32,
}

struct Play;

impl Service for Play {
    fn name(&self) -> &'static str {
        "play"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<StepParams>(db, "play.step", "Advance the play world.")
                .scope(Scope::Play)
                .action("ara_play", "step"),
        ]
    }

    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        _method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        let p: StepParams = ara_host::parse(params)?;
        let job = cx.start_job("play.step");
        cx.finish_job(&job, Ok(json!(p.ticks)));
        Ok(json!({ "tick": p.ticks, "author": cx.session().map(|s| s.author.to_string()) }))
    }
}

#[test]
fn services_register_methods_that_light_up_immediately() {
    let dir = project("service");
    let mut h = open(&dir, 9);
    let s = begin(&mut h, "agent:claude");
    let d = fail(&mut h, "play.step", json!({ "session": s, "ticks": 3 }));
    assert_eq!(d.code, codes::SERVICE_UNAVAILABLE);

    h.register(Box::new(Play)).expect("registers");
    let r = call(&mut h, "play.step", json!({ "session": s, "ticks": 3 }));
    assert_eq!(r, json!({ "tick": 3, "author": "agent:claude" }));
    let d = fail(&mut h, "play.step", json!({ "session": s, "ticks": -1 }));
    assert_eq!(
        d.code,
        codes::OUT_OF_RANGE,
        "params are validated before the service runs"
    );
    let info = h
        .catalog()
        .iter()
        .find(|m| m.name == "play.step")
        .expect("in the catalog");
    assert_eq!(info.service, "play");
    assert_eq!(
        info.params["properties"]["ticks"]["description"],
        "Ticks to advance."
    );
    assert!(
        h.register(Box::new(Play)).is_err(),
        "method names are unique"
    );
    let jobs = call(&mut h, "jobs.list", json!({ "session": s }));
    assert_eq!(jobs["jobs"][0]["result"], 3);
}
