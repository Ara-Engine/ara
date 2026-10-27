//! Exercise the complete version/package/smoke/sign/upload ordering without credentials.
use super::*;
use std::{cell::RefCell, rc::Rc};

type Calls = Rc<RefCell<Vec<(String, Vec<String>, bool)>>>;
struct Fixture {
    root: PathBuf,
    key: PathBuf,
    target: PathBuf,
    calls: Calls,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/preview-tests")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                crate::d26::validity::unix_ms().unwrap(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
        let root = directory.join("source");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = ['core']\n[workspace.package]\nversion = '0.0.1-alpha'\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("core/src")).unwrap();
        fs::write(
            root.join("core/Cargo.toml"),
            "[package]\nname='preview-fixture'\nversion.workspace=true\nedition='2024'\n",
        )
        .unwrap();
        fs::write(root.join("core/src/lib.rs"), "pub fn fixture() {}\n").unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n[[package]]\nname = 'ara'\nversion = '0.0.1-alpha'\n",
        )
        .unwrap();
        fs::write(root.join("RELEASE_NOTES.md"), "# Previous notes\n").unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
            vec!["add", "."],
            vec!["commit", "-m", "Fixture"],
        ] {
            crate::git::bytes(&root, args).unwrap();
        }
        let key = directory.join("key");
        fs::write(&key, "fake-secret-never-logged").unwrap();
        Self {
            root,
            key,
            target: directory.join("warm"),
            calls: Rc::default(),
        }
    }
    fn options(&self) -> Options {
        Options {
            publish: false,
            prepared: None,
            target_dir: Some(self.target.clone()),
            version: Some("0.0.2-alpha.1".into()),
            repo: Some("Ara-Engine/ara".into()),
            key: Some(self.key.clone()),
            json: false,
        }
    }
    fn runner(&self, fail: Option<&str>) -> Fake {
        Fake {
            calls: self.calls.clone(),
            fail: fail.map(str::to_owned),
        }
    }
    fn record(&self) -> PathBuf {
        self.root
            .join("target/ara-dev/previews/0.0.2-alpha.1/preview.json")
    }
}
struct Fake {
    calls: Calls,
    fail: Option<String>,
}
impl Runner for Fake {
    fn execute(&mut self, label: &str, command: &mut Command, gpu: bool) -> Result<()> {
        self.calls.borrow_mut().push((
            label.into(),
            command
                .get_args()
                .map(|s| s.to_string_lossy().into_owned())
                .collect(),
            gpu,
        ));
        ensure!(
            self.fail.as_deref() != Some(label),
            "fixture failure at {label}"
        );
        let root = command.get_current_dir().unwrap();
        if label == "lockfile" {
            // Run the real lockfile command: the version commit must carry its result.
            let update = Command::new("cargo")
                .current_dir(root)
                .args(command.get_args())
                .output()?;
            ensure!(update.status.success(), "fixture lockfile update failed");
        }
        if label == "release-tool" {
            let target = command
                .get_envs()
                .find(|(key, _)| *key == "CARGO_TARGET_DIR")
                .unwrap()
                .1
                .unwrap();
            let directory = Path::new(target)
                .join(std::env::var_os("CARGO_BUILD_TARGET").unwrap_or_default())
                .join("release");
            fs::create_dir_all(&directory)?;
            fs::write(
                directory.join(format!("ara-release{}", std::env::consts::EXE_SUFFIX)),
                "fixture tool",
            )?;
        }
        Ok(())
    }
}

#[test]
fn default_prepares_signed_artifacts_and_preserves_callers_workspace_and_refs() {
    let fixture = Fixture::new();
    let head = crate::git::text(&fixture.root, ["rev-parse", "HEAD"]).unwrap();
    workflow(&fixture.root, fixture.options(), fixture.runner(None)).unwrap();
    let calls = fixture.calls.borrow();
    assert_eq!(
        calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        [
            "lockfile",
            "release-tool",
            "samples-fetch",
            "samples-verify",
            "package",
            "smoke",
            "sign-hub",
            "sign-engine"
        ]
    );
    assert!(calls[5].2);
    assert!(
        calls[6..]
            .iter()
            .all(|c| c.1.iter().any(|arg| arg == "--dry-run"))
    );
    let text = fs::read_to_string(fixture.record()).unwrap();
    assert!(!text.contains("fake-secret"));
    let prepared: Prepared = serde_json::from_str(&text).unwrap();
    assert!(prepared.smoke_passed && !prepared.engine_published && !prepared.hub_published);
    assert_eq!(
        version::workspace_version(&fixture.root)
            .unwrap()
            .to_string(),
        "0.0.1-alpha"
    );
    assert_eq!(
        version::workspace_version(&prepared.source)
            .unwrap()
            .to_string(),
        "0.0.2-alpha.1"
    );
    assert_eq!(
        crate::git::text(&fixture.root, ["rev-parse", "HEAD"]).unwrap(),
        head
    );
    assert!(
        crate::git::text(&fixture.root, ["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        crate::git::text(&prepared.source, ["rev-parse", "HEAD^"]).unwrap(),
        head
    );
    // The bumped lockfile is in the version commit, so a later build leaves it clean.
    let committed_lock = crate::git::text(&prepared.source, ["show", "HEAD:Cargo.lock"]).unwrap();
    assert!(committed_lock.contains("version = \"0.0.2-alpha.1\""));
    assert!(
        crate::git::text(&prepared.source, ["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn failed_smoke_never_signs_or_pushes_even_with_publish_opt_in() {
    let fixture = Fixture::new();
    let mut options = fixture.options();
    options.publish = true;
    assert!(workflow(&fixture.root, options, fixture.runner(Some("smoke"))).is_err());
    assert_eq!(fixture.calls.borrow().last().unwrap().0, "smoke");
    assert!(!fixture.record().exists());
}

#[test]
fn resumed_publication_reverifies_before_source_push_then_hub_then_engine() {
    let fixture = Fixture::new();
    workflow(&fixture.root, fixture.options(), fixture.runner(None)).unwrap();
    fixture.calls.borrow_mut().clear();
    let mut options = fixture.options();
    options.publish = true;
    options.prepared = Some(fixture.record());
    options.version = None;
    options.repo = None;
    options.target_dir = None;
    workflow(&fixture.root, options, fixture.runner(None)).unwrap();
    let calls = fixture.calls.borrow();
    assert_eq!(
        calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        [
            "verify-hub",
            "verify-engine",
            "source-push",
            "publish-hub",
            "publish-engine"
        ]
    );
    assert!(
        calls[3..]
            .iter()
            .all(|c| !c.1.iter().any(|arg| arg == "--dry-run")
                && c.1.iter().any(|arg| arg == "--prerelease"))
    );
    let prepared: Prepared = serde_json::from_slice(&fs::read(fixture.record()).unwrap()).unwrap();
    assert!(prepared.hub_published && prepared.engine_published);
}
