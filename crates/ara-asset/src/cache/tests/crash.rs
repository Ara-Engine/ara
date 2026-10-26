//! Real kill boundaries plus explicit lost-write injection; no timing races.
use super::super::*;
use crate::{Cancellation, CookPreset, ImportContext, ImportInput, Importer, TargetPlatform};
use std::{
    collections::BTreeMap,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn input() -> ImportInput {
    ImportInput {
        source_path: "fixture.bin".into(),
        source: (0..65536).map(|i| (i as u8).wrapping_mul(17)).collect(),
        importer: "fixture.crash".into(),
        version: "1".into(),
        settings: serde_json::Value::Null,
        target: TargetPlatform::Windows,
        preset: CookPreset::Balanced,
        thread_budget: 1,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    }
}
struct Fixture;
impl Importer for Fixture {
    fn id(&self) -> &str {
        "fixture.crash"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn import(&self, input: &ImportInput, context: &mut ImportContext<'_>) -> Result<ImportOutput> {
        context.report(1.0, "fixture imported")?;
        Ok(ImportOutput {
            artifacts: vec![
                Artifact {
                    name: "primary".into(),
                    kind: ArtifactKind::Custom,
                    bytes: input.source.clone(),
                },
                Artifact {
                    name: "tail".into(),
                    kind: ArtifactKind::Custom,
                    bytes: input.source[..16].to_vec(),
                },
            ],
        })
    }
}
fn cook() -> ImportOutput {
    Fixture
        .import(
            &input(),
            &mut ImportContext {
                metrics: None,
                cancellation: &Cancellation::default(),
                progress: &mut |_| {},
            },
        )
        .expect("fixture cook")
}
struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "child fixture; invoked explicitly by the kill-mid-store test"]
fn crash_writer() {
    let root = std::env::var_os("ARA_TEST_CACHE_CRASH_ROOT").expect("parent fixture root");
    let cache = FileCache::at(root).expect("cache");
    cache
        .store(input().key().expect("key"), &cook())
        .expect("store");
}

#[test]
fn killed_stores_and_lost_writes_never_serve_partial_results() {
    let input = input();
    let key = input.key().expect("key");
    let expected = cook();
    for (stage, repair) in [
        ("mid-object-write", false),
        ("object-published", false),
        ("action-published", false),
        ("mid-object-write", true),
    ] {
        let root = tempfile::tempdir().expect("root");
        let cache = FileCache::at(root.path()).expect("cache");
        if repair {
            cache.store(key, &expected).expect("initial repair target");
            fs::write(cache.object(Digest::of(&input.source)), b"torn").expect("bad repair target");
        }
        let mut child = KillOnDrop(
            Command::new(std::env::current_exe().expect("owned test executable"))
                .args([
                    "--exact",
                    "cache::tests::crash::crash_writer",
                    "--ignored",
                    "--nocapture",
                ])
                .env("ARA_TEST_CACHE_CRASH_ROOT", root.path())
                .env("ARA_TEST_CACHE_CRASH_STAGE", stage)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("child"),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.path().join("paused").exists() {
            assert!(Instant::now() < deadline, "child did not reach {stage}");
            assert!(
                child.0.try_wait().expect("child state").is_none(),
                "child exited before {stage}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let before_kill = cache.load(key).expect("read at paused boundary");
        if stage == "action-published" {
            assert_eq!(before_kill.expect("complete action").output, expected);
        } else {
            assert!(
                before_kill.is_none(),
                "partial action became visible at {stage}"
            );
        }
        child.0.kill().expect("kill writer");
        child.0.wait().expect("reap writer");
        if stage == "action-published" {
            // Process termination does not discard the OS page cache. Explicitly
            // simulate a torn unflushed object with unchanged length after kill.
            let object = cache.object(Digest::of(&input.source));
            fs::write(object, vec![0xff; input.source.len()]).expect("lost object write");
        }
        assert!(cache.load(key).expect("loss is a miss").is_none());
        let mut imported = 0;
        let recooked = Fixture
            .import(
                &input,
                &mut ImportContext {
                    metrics: None,
                    cancellation: &Cancellation::default(),
                    progress: &mut |_| imported += 1,
                },
            )
            .expect("re-import after miss");
        assert_eq!(imported, 1);
        assert_eq!(recooked, expected);
        cache.store(key, &recooked).expect("repair after re-import");
        assert_eq!(
            cache
                .load(key)
                .expect("verified repair")
                .expect("hit")
                .output,
            expected
        );
    }
}
