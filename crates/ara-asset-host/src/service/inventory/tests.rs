use super::super::tests::{bindings, scratch, write_project};
use super::*;
use std::fs;

#[test]
fn project_policy_and_preset_parents_have_readonly_state_and_membership_proofs() {
    let scratch = scratch();
    let root = scratch.path();
    write_project(root);
    fs::create_dir_all(root.join("settings/builds")).expect("profile directory");
    fs::write(
        root.join(ara_host::project_policy::PREFERENCES),
        r#"{"main":"scenes/main.scene"}"#,
    )
    .expect("policy");
    fs::write(root.join("settings/builds/windows.export.json"), "{}").expect("profile");
    let bindings = bindings(root);
    let inventory = Inventory::read(root, &bindings, true, &Cancellation::default())
        .expect("readonly policy and profile inventory");

    for parent in ["settings", "settings/builds"] {
        assert_eq!(
            inventory.prepared.entries.get(parent),
            Some(&Some(FileState::directory())),
            "missing existing parent state: {parent}"
        );
    }
    assert_eq!(
        inventory.prepared.entries.get("settings/ara.imports"),
        Some(&None),
        "optional policy absence remains frozen"
    );
    assert_eq!(
        inventory.prepared.directories["settings"],
        vec!["settings/builds", ara_host::project_policy::PREFERENCES]
    );
    assert_eq!(
        inventory.prepared.directories["settings/builds"],
        vec!["settings/builds/windows.export.json"]
    );
    let history = FileHistory::new(root);
    history
        .revalidate(&inventory.prepared)
        .expect("unchanged inventory");
    assert!(!root.join(".ara").exists());

    fs::write(root.join("settings/added.json"), "{}").expect("concurrent settings file");
    let diagnostic = history
        .revalidate(&inventory.prepared)
        .expect_err("directory membership change must invalidate preparation");
    assert_eq!(diagnostic.code, codes::STALE_BASE);
    assert!(!root.join(".ara").exists());
}
