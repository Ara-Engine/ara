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
    Command::new(env!("CARGO_BIN_EXE_ara-dev"))
        .args(["gpu", "--timeout", &timeout.to_string(), "--"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "gpu_child", "--nocapture"])
        .env("LOCALAPPDATA", root)
        .env("ARA_GPU_TEST_ROOT", root)
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
