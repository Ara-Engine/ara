//! Services observe the Host's existing divergence state without changing it.
use super::*;
use crate::{Host, HostConfig, Project};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[test]
fn document_diverged_distinguishes_observed_invalid_policy_from_a_clean_document() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let target = std::path::absolute(target).expect("The test target has an absolute path");
    let root = target.join("host-service-divergence").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("settings")).expect("Create isolated policy fixture");
    std::fs::write(
        root.join("ara.project.json"),
        r#"{"name":"Divergence fixture"}"#,
    )
    .expect("Write marker fixture");
    let preferences = root.join(crate::project_policy::PREFERENCES);
    std::fs::write(&preferences, "{}").expect("Write clean policy fixture");
    let mut host = Host::open(
        Project::open(&root).expect("Open isolated fixture"),
        HostConfig {
            watch_every: Duration::ZERO,
            debounce: Duration::from_secs(3600),
            ..Default::default()
        },
    )
    .expect("Open fixture Host");
    {
        let cx = Cx {
            st: &mut host.st,
            conn: ConnId::LOCAL,
            session: None,
        };
        assert!(!cx.document_diverged("pj_preferences"));
        assert!(!cx.document_diverged("pj_project"));
    }
    std::fs::write(&preferences, "{broken").expect("Introduce a real external parse failure");
    host.tick();
    let revision = host.st.store.revision();
    {
        let cx = Cx {
            st: &mut host.st,
            conn: ConnId::LOCAL,
            session: None,
        };
        assert!(cx.document_diverged("pj_preferences"));
        assert!(!cx.document_diverged("pj_project"));
        assert!(!cx.document_diverged("pj_missing"));
    }
    assert_eq!(
        host.st.store.revision(),
        revision,
        "Reading divergence must not author a transaction"
    );
    assert_eq!(
        std::fs::read_to_string(&preferences).expect("Read protected policy"),
        "{broken"
    );
    drop(host);
    assert!(root.starts_with(&target));
    std::fs::remove_dir_all(root).expect("Remove only the isolated test fixture");
}
