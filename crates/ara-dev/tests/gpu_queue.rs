//! Subprocess tests exercise FIFO admission, timeout and stale ticket recovery.
#![allow(clippy::unwrap_used)]

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(Instant::now() < deadline, "GPU test timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wrapper(root: &Path, id: &str, timeout: u64) -> Child {
    wrapper_with_wait(root, id, timeout, false)
}

fn wrapper_with_wait(root: &Path, id: &str, timeout: u64, wait: bool) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ara-dev"))
        .args(["gpu", "--timeout", &timeout.to_string()])
        .args(if wait {
            vec!["--wait-for-quiet"]
        } else {
            vec![]
        })
        .arg("--")
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "gpu_child", "--nocapture"])
        .env("LOCALAPPDATA", root)
        .env("ARA_GPU_TEST_ROOT", root)
        .env("ARA_DEV_GPU_GUARD_FIXTURE", root.join("quiet.json"))
        .env("ARA_GPU_TEST_ID", id)
        .stdout(Stdio::null())
        .stderr(File::create(root.join(format!("{id}.log"))).unwrap())
        .spawn()
        .unwrap()
}

fn tickets(root: &Path) -> usize {
    fs::read_dir(root.join("ara/gpu-queue"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|e| e == "ticket"))
        .count()
}

fn lock(root: &Path) -> File {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("ara/gpu.lock"))
        .unwrap();
    file.lock().unwrap();
    file
}

#[test]
fn gpu_child() {
    let Some(root) = std::env::var_os("ARA_GPU_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let id = std::env::var("ARA_GPU_TEST_ID").unwrap();
    fs::write(root.join(format!("{id}.entered")), "entered").unwrap();
    wait_for(|| root.join(format!("{id}.release")).exists());
}

#[test]
fn fifo_timeout_and_crash_recovery() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-dev/tests")
        .join(format!(
            "gpu-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    fs::create_dir_all(root.join("ara")).unwrap();
    fs::write(root.join("quiet.json"), "[]").unwrap();
    let root = root.canonicalize().unwrap();
    let held = lock(&root);
    let mut first = wrapper(&root, "first", 15);
    wait_for(|| tickets(&root) == 1);
    let mut second = wrapper(&root, "second", 15);
    wait_for(|| tickets(&root) == 2);
    drop(held);
    wait_for(|| root.join("first.entered").exists());
    assert!(!root.join("second.entered").exists(), "FIFO was bypassed");
    fs::write(root.join("first.release"), "go").unwrap();
    assert!(first.wait().unwrap().success());
    wait_for(|| root.join("second.entered").exists());
    fs::write(root.join("second.release"), "go").unwrap();
    assert!(second.wait().unwrap().success());
    assert_eq!(tickets(&root), 0);

    let held = lock(&root);
    let mut crashed = wrapper(&root, "crashed", 15);
    wait_for(|| tickets(&root) == 1);
    crashed.kill().unwrap();
    crashed.wait().unwrap();
    assert_eq!(
        tickets(&root),
        1,
        "killed waiter should leave a stale ticket"
    );
    let mut timeout = wrapper(&root, "timeout", 0);
    assert!(!timeout.wait().unwrap().success());
    drop(held);
    fs::write(root.join("recovery.release"), "go").unwrap();
    assert!(wrapper(&root, "recovery", 15).wait().unwrap().success());
    assert_eq!(tickets(&root), 0);
}

#[test]
fn foreign_load_refuses_start_and_invalidates_a_running_lease() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-dev/tests")
        .join(format!("gpu-load-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let blocked = serde_json::json!({"unix_ms":1000,"processes":[{"pid":9999,"parent":0,"name":"VALORANT-Win64-Shipping.exe","birth":1}],"engines":[]});
    fs::write(
        root.join("quiet.json"),
        serde_json::to_vec(&vec![blocked; 3]).unwrap(),
    )
    .unwrap();
    let output = wrapper(&root, "blocked", 15).wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(!root.join("blocked.entered").exists());
    assert!(
        fs::read_to_string(root.join("blocked.log"))
            .unwrap()
            .contains("VALORANT-Win64-Shipping.exe")
    );

    let mut frames: Vec<_> = [1000, 2000, 3000]
        .into_iter()
        .map(|at| serde_json::json!({"unix_ms":at,"processes":[],"engines":[]}))
        .collect();
    for at in [4000, 5000, 6000, 7000, 8000] {
        frames.push(serde_json::json!({"unix_ms":at,"processes":[{"pid":9998,"parent":0,"name":"foreign-game.exe","birth":1}],"engines":[["pid_9998_luid_0_eng_0",23.]]}));
    }
    fs::write(
        root.join("quiet.json"),
        serde_json::to_vec(&frames).unwrap(),
    )
    .unwrap();
    let mut reading = wrapper(&root, "reading", 15);
    wait_for(|| root.join("reading.entered").exists());
    wait_for(|| {
        fs::read_to_string(root.join("reading.log")).is_ok_and(|s| s.contains("INVALID reading"))
    });
    fs::write(root.join("reading.release"), "go").unwrap();
    assert_eq!(reading.wait().unwrap().code(), Some(3));
    let log = fs::read_to_string(root.join("reading.log")).unwrap();
    let line = log
        .lines()
        .find_map(|l| l.strip_prefix("ARA_DEV_GPU_READING "))
        .unwrap();
    let report: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(report["status"], "invalid");
    assert_eq!(report["reading"]["contaminants"][0]["peak_percent"], 23.);
    assert_eq!(report["reading"]["contaminants"][0]["first_unix_ms"], 3000);
}

#[test]
fn wait_for_quiet_starts_only_after_the_contaminant_clears() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-dev/tests")
        .join(format!("gpu-wait-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let busy = serde_json::json!({"unix_ms":1000,"processes":[{"pid":9997,"parent":0,"name":"VALORANT-Win64-Shipping.exe","birth":1}],"engines":[]});
    let mut frames = vec![busy; 3];
    for at in [4000, 5000, 6000] {
        frames.push(serde_json::json!({"unix_ms":at,"processes":[],"engines":[]}));
    }
    fs::write(
        root.join("quiet.json"),
        serde_json::to_vec(&frames).unwrap(),
    )
    .unwrap();
    fs::write(root.join("wait.release"), "go").unwrap();
    let started = Instant::now();
    let mut child = wrapper_with_wait(&root, "wait", 15, true);
    assert!(child.wait().unwrap().success());
    assert!(started.elapsed() >= Duration::from_secs(6));
    assert!(root.join("wait.entered").exists());
    let text = fs::read_to_string(root.join("wait.log")).unwrap();
    let value: serde_json::Value = serde_json::from_str(
        text.lines()
            .find_map(|l| l.strip_prefix("ARA_DEV_GPU_READING "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["status"], "test_only");
    assert!(
        value["reading"]["contaminants"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
