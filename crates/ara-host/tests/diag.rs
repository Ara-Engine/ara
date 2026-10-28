//! The diagnostics bus through the Engine Protocol: `logs` summaries, `diag.query`,
//! `diag.subscribe` streams (host logs and other threads' emits), `diag.clear` and
//! `diag.report`. The bus is process-global, so these tests run one at a time.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ara_diag::bus::Meta;
use ara_diag::{Code, Diagnostic, Subject, codes};
use ara_host::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::{Value, json};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
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

fn open(name: &str) -> Host {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project");
    let dir: PathBuf = std::env::temp_dir().join(format!("ara-diag-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&fixture, &dir);
    let config = HostConfig {
        debounce: Duration::from_secs(3600),
        watch_every: Duration::from_secs(3600),
        scan_every: Duration::from_secs(3600),
        seed: Some(7),
        ..HostConfig::default()
    };
    Host::open(Project::open(&dir).expect("project"), config).expect("host")
}

fn call(h: &mut Host, conn: ConnId, method: &str, params: Value) -> Value {
    h.call(conn, method, params)
        .unwrap_or_else(|e| panic!("{method} failed:\n{e}"))
}

fn begin(h: &mut Host, author: &str) -> String {
    let r = call(
        h,
        ConnId::LOCAL,
        "session.begin",
        json!({ "client": "test", "author": author }),
    );
    r["session"].as_str().expect("handle").to_owned()
}

/// Makes the host log one error per bad `inspect`: ARA1204 about `entity`.
fn fail_inspect(h: &mut Host, s: &str, entity: &str) {
    let _ = h.call(
        ConnId::LOCAL,
        "inspect",
        json!({ "session": s, "entities": [entity] }),
    );
}

#[test]
fn logs_are_grouped_budgeted_and_attributed() {
    let _g = serial();
    let mut h = open("logs");
    let agent = begin(&mut h, "agent:codex");
    for e in ["e_nope01", "e_nope02", "e_nope01"] {
        fail_inspect(&mut h, &agent, e);
    }
    let r = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": agent, "codes": ["ARA1204"] }),
    );
    let text = r["text"].as_str().expect("text");
    assert!(
        text.starts_with("2 errors Â· 0 warnings Â· 0 info (3 occurrences)"),
        "{text}"
    );
    assert!(
        text.contains("error ARA1204") && text.contains("Ã—3 [entity:e_nope01"),
        "{text}"
    );
    assert!(text.contains("explain: ara why ARA1204"), "{text}");
    assert!(r.get("summary").is_none(), "agents get the text only");
    let full = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": agent, "codes": ["ARA1204"], "limit": 10 }),
    );
    assert_eq!(full["summary"]["groups"][0]["records"], 2);

    let mine = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": agent, "origin": "agent:codex" }),
    );
    assert_eq!(
        mine["total"], 2,
        "errors are attributed to the calling agent"
    );
    let theirs = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": agent, "origin": "human" }),
    );
    assert_eq!(theirs["total"], 0);

    let about = call(
        &mut h,
        ConnId::LOCAL,
        "diag.query",
        json!({ "session": agent, "subjects": ["entity:e_nope02"], "group": false }),
    );
    assert_eq!(about["entries"].as_array().map(Vec::len), Some(1));
    assert!(
        about["text"]
            .as_str()
            .is_some_and(|t| t.contains("e_nope02"))
    );

    let bad = h
        .call(
            ConnId::LOCAL,
            "logs",
            json!({ "session": agent, "codes": ["ARA13O2"] }),
        )
        .expect_err("bad code");
    let d = &bad.diagnostics()[0];
    assert_eq!(d.code, codes::UNKNOWN_CODE);
    assert!(
        d.message.contains("did you mean `ARA1302`?"),
        "{}",
        d.message
    );
}

#[test]
fn subscriptions_stream_host_logs_and_other_threads_emits() {
    let _g = serial();
    let mut h = open("subscribe");
    let s = begin(&mut h, "human");
    let conn = ConnId(9);
    let sub = call(
        &mut h,
        conn,
        "diag.subscribe",
        json!({ "session": s, "subjects": ["asset:@meshes/crate.glb"], "severity": "warning" }),
    );
    let id = sub["subscription"]
        .as_str()
        .expect("subscription")
        .to_owned();

    // Another thread (an import worker) reports on the bus; the host drains it on tick.
    std::thread::spawn(|| {
        Diagnostic::error(codes::ASSET_IMPORT_FAILED, "`crate.glb` has no meshes")
            .subject(Subject::asset("@meshes/crate.glb#ms_crate001"))
            .emit()
            .expect("admitted");
        Diagnostic::error(codes::ASSET_IMPORT_FAILED, "`lid.glb` has no meshes")
            .subject(Subject::asset("@meshes/lid.glb"))
            .emit();
    })
    .join()
    .expect("worker");
    fail_inspect(&mut h, &s, "e_unrelated");
    h.tick();

    let events: Vec<Value> = h
        .drain_outbox()
        .into_iter()
        .filter(|(c, m)| *c == conn && m["method"] == "diag.event")
        .map(|(_, m)| m["params"].clone())
        .collect();
    assert_eq!(
        events.len(),
        1,
        "only the crate import matches: {events:#?}"
    );
    assert_eq!(events[0]["subscription"], id.as_str());
    assert_eq!(events[0]["record"]["diagnostic"]["code"], "ARA2101");
    assert_eq!(events[0]["record"]["origin"], "engine");

    call(
        &mut h,
        conn,
        "diag.unsubscribe",
        json!({ "session": s, "subscription": id }),
    );
    std::thread::spawn(|| {
        Diagnostic::error(codes::ASSET_IMPORT_FAILED, "again")
            .subject(Subject::asset("@meshes/crate.glb"))
            .emit();
    })
    .join()
    .expect("worker");
    h.tick();
    assert!(
        !h.drain_outbox()
            .iter()
            .any(|(_, m)| m["method"] == "diag.event"),
        "no events after unsubscribing"
    );
}

#[test]
fn other_processes_report_and_records_can_be_cleared() {
    let _g = serial();
    let mut h = open("report");
    let s = begin(&mut h, "human");
    let r = call(
        &mut h,
        ConnId::LOCAL,
        "diag.report",
        json!({
            "session": s,
            "engine": true,
            "tick": 120,
            "diagnostics": [
                { "code": "ARA3304", "severity": "error", "message": "`Crate` has a NaN position", "location": { "record": "e_crate1" } },
                { "code": "ARA_GPU_VALIDATION", "severity": "warning", "message": "VUID-vkCmdDraw-None-08600" },
                { "code": "nope", "severity": "error", "message": "x" },
            ],
        }),
    );
    assert_eq!(r["recorded"], 2);
    assert_eq!(r["skipped"][0]["code"], "ARA2002");
    let nan = call(
        &mut h,
        ConnId::LOCAL,
        "diag.query",
        json!({ "session": s, "subjects": ["e_crate1"], "fromTick": 100, "group": false }),
    );
    assert_eq!(nan["entries"][0]["tick"], 120);
    let gpu = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": s, "codes": ["ARA_GPU_VALIDATION"] }),
    );
    assert!(
        gpu["text"]
            .as_str()
            .is_some_and(|t| t.contains("warning ARA_GPU_VALIDATION Vulkan validation message"))
    );

    let cleared = call(
        &mut h,
        ConnId::LOCAL,
        "diag.clear",
        json!({ "session": s, "codes": ["ARA3304"] }),
    );
    assert_eq!(cleared["removed"], 1);
    let left = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": s, "codes": ["ARA3304"] }),
    );
    assert_eq!(left["total"], 0);
}

#[test]
fn why_explains_through_the_protocol_and_its_fixes_run() {
    let _g = serial();
    let mut h = open("why");
    let s = begin(&mut h, "agent:codex");
    call(
        &mut h,
        ConnId::LOCAL,
        "cmd.apply",
        json!({ "session": s, "why": "Fade the lid out", "commands": [
            { "cmd": "property.set", "args": { "entity": "e_stk2/e_c001/e_lid1", "path": "MeshRenderer.tint", "value": [0.2, 0.9, 0.2, 0] } },
        ]}),
    );
    let r = call(
        &mut h,
        ConnId::LOCAL,
        "why",
        json!({ "session": s, "question": "why is e_stk2/e_c001/e_lid1 not visible", "full": true }),
    );
    assert_eq!(r["explainer"], "visibility");
    assert_eq!(r["causes"][0]["code"], "ARA3307", "{}", r["text"]);
    assert_eq!(r["causes"][0]["likelihood"], "high");
    let text = r["text"].as_str().expect("text");
    assert!(text.contains("`MeshRenderer.tint` has alpha 0"), "{text}");

    // The fix is a command: run it as is, and the cause is gone.
    let fix = &r["causes"][0]["fixes"][0];
    assert_eq!(fix["action"], "command");
    call(
        &mut h,
        ConnId::LOCAL,
        "cmd.apply",
        json!({ "session": s, "why": "Make the lid visible again", "commands": [{ "cmd": fix["command"], "args": fix["params"] }] }),
    );
    let r = call(
        &mut h,
        ConnId::LOCAL,
        "why",
        json!({ "session": s, "question": "why is e_stk2/e_c001/e_lid1 not visible", "full": true }),
    );
    assert!(
        r["causes"]
            .as_array()
            .is_some_and(|c| c.iter().all(|c| c["likelihood"] != "high")),
        "{}",
        r["text"]
    );

    let list = call(
        &mut h,
        ConnId::LOCAL,
        "why",
        json!({ "session": s, "list": true }),
    );
    assert!(
        list["text"]
            .as_str()
            .is_some_and(|t| t.contains("visibility") && t.contains("reload"))
    );
    let none = h
        .call(
            ConnId::LOCAL,
            "why",
            json!({ "session": s, "question": "why is name:Nothing not visible" }),
        )
        .expect_err("unknown entity");
    assert_eq!(none.diagnostics()[0].code, codes::RECORD_NOT_FOUND);
}

#[test]
fn engine_diagnostics_survive_a_restart_with_their_causes() {
    let _g = serial();
    let mut h = open("persist");
    let s = begin(&mut h, "human");
    std::thread::spawn(|| {
        let parent = Diagnostic::error(Code(7001), "rigid body `Lamp` has zero mass")
            .subject(Subject::entity("e_lmp1"))
            .emit();
        Diagnostic::error(
            codes::RENDER_NON_FINITE_TRANSFORM,
            "`Lamp` has a NaN position",
        )
        .subject(Subject::entity("e_lmp1"))
        .emit_with(Meta::caused_by(parent));
    })
    .join()
    .expect("worker");
    let _ = h.call(
        ConnId::LOCAL,
        "inspect",
        json!({ "session": s, "entities": ["e_typo"] }),
    );
    h.tick();
    let root = h.root().to_path_buf();
    drop(h);

    let config = HostConfig {
        seed: Some(8),
        ..HostConfig::default()
    };
    let mut h = Host::open(Project::open(&root).expect("project"), config).expect("reopen");
    let s = begin(&mut h, "human");
    let nan = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": s, "codes": ["ARA3304"], "group": false }),
    );
    assert_eq!(nan["total"], 1, "the engine's report survived");
    let typo = call(
        &mut h,
        ConnId::LOCAL,
        "logs",
        json!({ "session": s, "codes": ["ARA1204"] }),
    );
    assert_eq!(typo["total"], 0, "call errors are not kept");
    let r = call(
        &mut h,
        ConnId::LOCAL,
        "why",
        json!({ "session": s, "question": "why is e_lmp1 not visible" }),
    );
    let text = r["text"].as_str().expect("text");
    assert!(
        text.contains("caused by: error ARA7001 rigid body `Lamp` has zero mass"),
        "{text}"
    );
}
