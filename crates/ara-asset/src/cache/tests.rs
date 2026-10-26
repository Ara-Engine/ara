use super::*;
mod crash;

thread_local! {
    static PUBLISH_FAILURE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub(super) fn fail_publish(after: bool) -> std::io::Result<()> {
    let fail = PUBLISH_FAILURE.with(|state| {
        if state.get() == Some(after) {
            if after {
                state.set(None);
            }
            true
        } else {
            false
        }
    });
    if fail {
        Err(std::io::Error::from_raw_os_error(5))
    } else {
        Ok(())
    }
}

#[test]
fn failed_publication_only_succeeds_when_the_live_bytes_verify() {
    for after in [false, true] {
        let root = tempfile::tempdir().expect("root");
        let cache = FileCache::at(root.path()).expect("cache");
        let key = Digest::of(b"publication race");
        PUBLISH_FAILURE.with(|state| state.set(Some(after)));
        let result = cache.store(key, &output());
        PUBLISH_FAILURE.with(|state| state.set(None));
        if after {
            result.expect("complete racing publication is usable");
            assert_eq!(
                cache.load(key).expect("read").expect("hit").output,
                output()
            );
        } else {
            assert!(matches!(result, Err(Error::Io(error)) if error.raw_os_error() == Some(5)));
            assert!(cache.load(key).expect("nothing published").is_none());
        }
    }
}

fn output() -> ImportOutput {
    ImportOutput {
        artifacts: vec![Artifact {
            name: "data".into(),
            kind: ArtifactKind::Custom,
            bytes: vec![1, 2, 3],
        }],
    }
}

#[test]
fn intermediate_and_final_cache_verify_and_repair_independently() {
    let root = tempfile::tempdir().expect("root");
    let final_cache = FileCache::at(root.path()).expect("final cache");
    let memo = FileCache::intermediates(root.path()).expect("memo");
    let key = Digest::of(b"key");
    let output = output();
    memo.store(key, &output).expect("intermediate");
    assert!(final_cache.load(key).expect("separate namespace").is_none());
    final_cache.store(key, &output).expect("final");
    let hash = Digest::of(&output.artifacts[0].bytes);
    fs::remove_file(memo.object(hash)).expect("lose memo name");
    fs::write(memo.object(hash), b"broken").expect("simulate lost memo data");
    assert!(memo.load(key).expect("damaged intermediate").is_none());
    memo.store(key, &output).expect("recompute");
    let gc = final_cache
        .collect(&BTreeSet::from([key]))
        .expect("quiescent GC");
    assert_eq!(gc.actions_removed, 1);
    assert!(memo.load(key).expect("memo evicted").is_none());
    assert_eq!(
        final_cache
            .load(key)
            .expect("final hit")
            .expect("root")
            .output,
        output
    );
    fs::write(final_cache.object(hash), b"broken").expect("damage final object");
    assert!(final_cache.load(key).expect("damage is a miss").is_none());
    final_cache
        .store(key, &output)
        .expect("repair final object");
    assert_eq!(
        final_cache
            .load(key)
            .expect("repaired hit")
            .expect("root")
            .output,
        output
    );
}

#[test]
fn concurrent_repairs_do_not_remove_or_serve_bad_data() {
    let root = tempfile::tempdir().expect("root");
    let cache = FileCache::at(root.path()).expect("cache");
    let key = Digest::of(b"concurrent repair");
    let expected = output();
    cache.store(key, &expected).expect("initial");
    fs::write(cache.action(key), b"{\"version\":").expect("torn action");
    let hash = Digest::of(&expected.artifacts[0].bytes);
    fs::write(cache.object(hash), [9, 9, 9]).expect("same-length corrupt object");
    let barrier = std::sync::Barrier::new(12);
    std::thread::scope(|scope| {
        for _ in 0..12 {
            scope.spawn(|| {
                barrier.wait();
                for _ in 0..20 {
                    if let Some(hit) = cache.load(key).expect("concurrent read") {
                        assert_eq!(hit.output, expected);
                    }
                    cache.store(key, &expected).expect("concurrent repair");
                }
            });
        }
    });
    assert_eq!(
        cache.load(key).expect("final hit").expect("root").output,
        expected
    );
}

#[test]
fn matching_hash_does_not_bypass_payload_validation() {
    let root = tempfile::tempdir().expect("root");
    let cache = FileCache::at(root.path()).expect("cache");
    let key = Digest::of(b"invalid mesh cache record");
    let bytes = b"not a cooked mesh";
    let hash = Digest::of(bytes);
    fs::write(cache.object(hash), bytes).expect("bad payload with a matching hash");
    let entry = Entry {
        version: 1,
        artifacts: vec![Blob {
            name: "mesh".into(),
            kind: ArtifactKind::Mesh,
            hash,
            length: bytes.len() as u64,
        }],
    };
    let text = ara_json::to_canonical_string(&ara_json::sort_keys(
        &serde_json::to_value(entry).expect("entry"),
    ));
    fs::write(cache.action(key), text).expect("action");
    assert!(
        cache
            .load(key)
            .expect("invalid payload is a miss")
            .is_none()
    );
    let expected = output();
    cache
        .store(key, &expected)
        .expect("recook replaces the invalid record");
    assert_eq!(
        cache
            .load(key)
            .expect("verified repair")
            .expect("hit")
            .output,
        expected
    );
}

pub(super) fn pause_at(stage: &str, path: &Path) -> bool {
    let Ok(requested) = std::env::var("ARA_TEST_CACHE_CRASH_STAGE") else {
        return false;
    };
    let Some(root) = std::env::var_os("ARA_TEST_CACHE_CRASH_ROOT") else {
        return false;
    };
    let folder = if stage == "action-published" {
        "actions"
    } else {
        "objects"
    };
    requested == stage
        && path.starts_with(Path::new(&root))
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == folder)
}

pub(super) fn pause(path: &Path) {
    use std::io::Read;
    let root = std::env::var_os("ARA_TEST_CACHE_CRASH_ROOT").expect("fixture root");
    fs::write(
        Path::new(&root).join("paused"),
        path.to_string_lossy().as_bytes(),
    )
    .expect("signal parent");
    // The parent holds stdin open until it kills us. Parent failure closes the
    // pipe and lets this fixture terminate instead of leaving an orphan.
    let _ = std::io::stdin().read_exact(&mut [0]);
}
