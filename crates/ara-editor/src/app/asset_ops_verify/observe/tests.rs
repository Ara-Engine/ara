//! Asset receipts use logical references; exact snapshots retain storage namespaces.
use super::*;

#[test]
fn move_checks_resolve_snapshot_storage_keys_without_changing_reference_paths() {
    for modern in [false, true] {
        let mut workspace = ara_doc::Workspace::new(ara_doc::AssetIndex::new());
        if modern {
            workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        }
        for folder in ["P4 Asset Proof/Rename", "game/P4 Asset Proof/Rename"] {
            let source = format!("{folder}/A.prefab");
            let destination = format!("{folder}/Renamed.prefab");
            let source_storage = workspace.storage_path(&source);
            let destination_storage = workspace.storage_path(&destination);
            let old_reference = format!("@{source}#pf_nativeproof");
            let new_reference = format!("@{destination}#pf_nativeproof");
            let before = json!({"documents":{(source_storage.clone()):{"id":"pf_nativeproof","bytes":217,"digest":"exact canonical digest"}},"references":[old_reference]});
            let after = json!({"documents":{(destination_storage.clone()):{"id":"pf_nativeproof","bytes":217,"digest":"exact canonical digest"}},"references":[new_reference]});
            let moved = moved_document(&before, &after, &workspace, &source, &destination);
            assert!(
                moved.ok(),
                "{}",
                serde_json::to_string(&moved).expect("asset proof serializes")
            );
            assert_eq!(moved.source_storage_path, source_storage);
            assert_eq!(moved.destination_storage_path, destination_storage);
            assert_eq!(reference(&before, &workspace, &source), Some(old_reference));
            assert_eq!(
                reference(&after, &workspace, &destination),
                Some(new_reference.clone())
            );
            assert_eq!(
                document(&before, &workspace, &source),
                document(&after, &workspace, &destination)
            );
            assert!(document(&after, &workspace, &source).is_null());

            let mut wrong_id = after.clone();
            wrong_id["documents"][&destination_storage]["id"] = json!("pf_otherproof");
            assert!(
                !moved_document(&before, &wrong_id, &workspace, &source, &destination).id_preserved
            );
            assert!(!moved_document(&before, &wrong_id, &workspace, &source, &destination).ok());

            let mut stale_reference = after.clone();
            stale_reference["references"] = before["references"].clone();
            let checks =
                moved_document(&before, &stale_reference, &workspace, &source, &destination);
            assert!(!checks.old_reference_removed);
            assert!(!checks.new_reference_present);
            assert!(!checks.ok());

            let mut copied = after.clone();
            copied["documents"][&source_storage] = before["documents"][&source_storage].clone();
            assert!(
                !moved_document(&before, &copied, &workspace, &source, &destination).source_removed
            );
            assert!(!moved_document(&before, &copied, &workspace, &source, &destination).ok());

            let missing = json!({"documents":{},"references":[new_reference]});
            let checks = moved_document(&missing, &after, &workspace, &source, &destination);
            assert!(!checks.source_present);
            assert!(!checks.id_preserved);
            assert!(
                !checks.ok(),
                "Missing IDs must never compare equal through null indexing"
            );
        }
    }
}
