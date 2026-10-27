//! The host thread never waits on the disk: journal and diagnostics writes queue on
//! the file thread, and `Host::flush` is the point that waits for them.
mod why_paths;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_proto::ConnId;
use serde_json::{Value, json};

use super::{Host, HostConfig};
use crate::Project;

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

/// A fresh copy of the fixture project.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-host-unit-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project"),
        &dir,
    );
    dir
}

fn call(host: &mut Host, method: &str, params: Value) -> Value {
    host.call(ConnId::LOCAL, method, params)
        .unwrap_or_else(|e| panic!("{method} failed:\n{e}"))
}

#[test]
#[cfg(unix)]
fn shutdown_releases_project_ownership_even_with_a_retained_os_handle() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/host-lock-tests")
        .join(std::process::id().to_string());
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project"),
        &dir,
    );
    let host = Host::open(Project::open(&dir).expect("project"), HostConfig::default())
        .expect("first host");
    let retained = host.st._lock.0.try_clone().expect("retained OS reference");
    host.kill();
    let reopened = Host::open(Project::open(&dir).expect("project"), HostConfig::default());
    assert!(
        reopened.is_ok(),
        "shutdown must release ownership before a retained OS reference disappears: {:?}",
        reopened.err()
    );
    drop(retained);
}

/// While the file thread is held (a disk stalled by an import), a command and a
/// diagnostics rewrite still return at once; the journal gets the transaction, in
/// commit order, once the thread runs again.
#[test]
fn commands_return_while_the_file_thread_is_held() {
    let dir = project("held");
    let config = HostConfig {
        debounce: Duration::from_secs(3600),
        watch_every: Duration::ZERO,
        scan_every: Duration::ZERO,
        seed: Some(91),
        ..HostConfig::default()
    };
    let mut host = Host::open(Project::open(&dir).expect("project"), config).expect("host");
    let session = call(
        &mut host,
        "session.begin",
        json!({ "client": "test", "author": "agent:held" }),
    )["session"]
        .as_str()
        .expect("handle")
        .to_owned();
    host.flush();
    let journal = dir.join(".ara/journal");

    let release = host.st.files.io.hold();
    let at = Instant::now();
    let applied = call(
        &mut host,
        "cmd.apply",
        json!({ "session": session, "doc": "levels/arena.scene", "why": "rename the floor",
                "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Ground" } }] }),
    );
    call(
        &mut host,
        "diag.report",
        json!({ "session": session, "engine": true, "diagnostics": [
            { "code": "ARA3304", "severity": "error", "message": "`Crate` has a NaN position" }
        ] }),
    );
    let cleared = call(
        &mut host,
        "diag.clear",
        json!({ "session": session, "codes": ["ARA3304"] }),
    );
    let elapsed = at.elapsed();
    let txn = applied["txn"].as_str().expect("txn").to_owned();
    // The hold lasts up to 30 s: a host that waited for it would take that long.
    assert!(
        elapsed < Duration::from_secs(5),
        "the host waited for the held file thread: {elapsed:?}"
    );
    assert_eq!(cleared["removed"], 1);
    assert!(
        !fs::read_to_string(&journal)
            .expect("journal")
            .contains(&txn),
        "the append ran on the host thread"
    );

    drop(release);
    host.flush();
    let text = fs::read_to_string(&journal).expect("journal");
    let line = |needle: &str| {
        text.lines()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no journal line with {needle}"))
    };
    assert!(
        line(r#""t":"session""#) < line(&txn),
        "the session entry precedes its transaction"
    );
    drop(host);
    let _ = fs::remove_dir_all(&dir);
}
