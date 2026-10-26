//! Real filesystem watcher tests use a disposable source tree under target/.
#![allow(clippy::unwrap_used)]
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

struct Resources(PathBuf);
impl Resources {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let root = target.join(format!(
            "style-watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("assets/themes")).unwrap();
        std::fs::create_dir_all(root.join("crates/ara-editor/src/shell")).unwrap();
        for (name, theme) in [
            ("ara-dark.json", EditorTheme::Dark),
            ("ara-light.json", EditorTheme::Light),
        ] {
            std::fs::write(root.join("assets/themes").join(name), theme.json()).unwrap();
        }
        std::fs::write(root.join("crates/ara-editor/src/shell/editor.css"),"ui { width:100%; height:100%; background:var(--card); font-size:var(--font-size-body); }").unwrap();
        Self(root)
    }
    fn metric(&self, size: u32) {
        let mut value: Value = ara_json::parse(EditorTheme::Dark.json()).unwrap();
        value["metrics"]["control-size"] = size.into();
        std::fs::write(
            self.0.join("assets/themes/ara-dark.json"),
            ara_json::to_canonical_string(&value),
        )
        .unwrap();
    }
}
impl Drop for Resources {
    fn drop(&mut self) {
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("style-watch-")
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn watcher_publishes_valid_saves_rejects_invalid_saves_and_recovers() {
    let resources = Resources::new();
    let (updates, stop) = watch(resources.0.clone()).unwrap();
    let receive = || updates.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(receive().unwrap().tokens[0]["--control-size"], "24px");
    resources.metric(30);
    assert_eq!(receive().unwrap().tokens[0]["--control-size"], "30px");
    std::fs::write(
        resources.0.join("assets/themes/ara-dark.json"),
        "{ unfinished",
    )
    .unwrap();
    assert!(receive().is_err());
    resources.metric(28);
    assert_eq!(receive().unwrap().tokens[0]["--control-size"], "28px");
    std::fs::write(
        resources.0.join("crates/ara-editor/src/shell/editor.css"),
        "ui { unsupported:10px; }",
    )
    .unwrap();
    assert!(receive().is_err());
    drop(stop);
}

#[test]
fn invalid_update_leaves_last_good_resource_snapshot_in_place() {
    let resources = Resources::new();
    let (tx, updates) = std::sync::mpsc::channel();
    let (stop, _stopped) = std::sync::mpsc::channel();
    let mut styles = LiveStyles {
        css: BTreeMap::new(),
        tokens: [
            EditorTheme::Dark.tokens().unwrap(),
            EditorTheme::Light.tokens().unwrap(),
        ],
        updates,
        _stop: stop,
    };
    resources.metric(31);
    tx.send(read_snapshot(&resources.0)).unwrap();
    assert!(styles.poll().unwrap());
    tx.send(Err("incomplete save".into())).unwrap();
    assert!(styles.poll().is_err());
    assert_eq!(styles.tokens(false)["--control-size"], "31px");
    assert!(!styles.poll().unwrap());
}
