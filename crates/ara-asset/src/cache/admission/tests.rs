use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn isolated(root: &Path, limit: u32) -> Volume {
    let volume = Volume {
        directory: root.to_owned(),
    };
    let mut state = open(&root.join("policy")).expect("test state");
    let mut policy = Policy::new(now_ms());
    policy.limit = limit;
    save(&mut state, policy).expect("initial limit");
    volume
}

#[test]
fn project_roots_on_the_same_volume_share_admission() {
    let a = tempfile::tempdir().expect("project A");
    let b = tempfile::tempdir().expect("project B");
    assert_eq!(
        Volume::new(a.path()).expect("volume A").directory,
        Volume::new(b.path()).expect("volume B").directory
    );
}

#[test]
fn concurrent_threads_stay_within_the_limit_and_removed_slots_drain() {
    let root = tempfile::tempdir().expect("isolated coordinator");
    let volume = isolated(root.path(), 2);
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..6 {
            scope.spawn(|| {
                for _ in 0..5 {
                    let permit = volume.acquire().expect("lease");
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(1));
                    active.fetch_sub(1, Ordering::SeqCst);
                    permit.finish(3).expect("release");
                }
            });
        }
    });
    assert!(peak.load(Ordering::SeqCst) <= 2);
    let a = volume.acquire().expect("first");
    let b = volume.acquire().expect("second");
    let mut state = open(&root.path().join("policy")).expect("feedback");
    state.lock().expect("transaction");
    let mut policy = load(&mut state).expect("load");
    policy.limit = 1;
    save(&mut state, policy).expect("shrink");
    drop(state);
    drop(a);
    assert!(
        volume.try_acquire().expect("admit").is_none(),
        "all old slots count after shrink"
    );
    drop(b);
    assert!(volume.try_acquire().expect("admit").is_some());
}

#[test]
fn child_holds_slot() {
    let Some(root) = std::env::var_os("ARA_TEST_WRITE_ADMISSION") else {
        return;
    };
    let root = Path::new(&root);
    let volume = Volume {
        directory: root.to_owned(),
    };
    let _permit = volume.acquire().expect("child lease");
    fs::write(root.join("ready"), []).expect("notify parent");
    // Parent exit closes stdin; a failed assertion must not orphan the fixture.
    let _ = std::io::stdin().read_exact(&mut [0]);
}

#[test]
fn separate_processes_share_the_cap_and_killed_writers_release_capacity() {
    use std::process::{Command, Stdio};
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().expect("isolated coordinator");
    let volume = isolated(root.path(), 1);
    let mut child = Child(
        Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "cache::admission::tests::child_holds_slot",
                "--nocapture",
            ])
            .env("ARA_TEST_WRITE_ADMISSION", root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("child"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !root.path().join("ready").exists() {
        assert!(Instant::now() < deadline, "child admission deadline");
        assert!(
            child.0.try_wait().expect("child status").is_none(),
            "child exited before acquiring"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(volume.try_acquire().expect("parent admission").is_none());
    child.0.kill().expect("simulate worker crash");
    child.0.wait().expect("reap worker");
    assert!(volume.try_acquire().expect("crash recovery").is_some());
}
