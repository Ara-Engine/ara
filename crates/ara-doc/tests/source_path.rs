//! Sidecars retain their save path without redirecting media references.

use ara_doc::{AssetId, Document, Workspace, refs};

#[test]
fn sidecar_save_path_is_independent_of_reference_path() {
    let mut ws = Workspace::default();
    let id = AssetId::parse("as_mesh0001").expect("valid fixture");
    ws.insert_with_asset_path(
        "models/mesh.glb.import.json",
        Document::new("import", id),
        "models/mesh.glb",
    );
    assert_eq!(
        ws.path_of("as_mesh0001"),
        Some("models/mesh.glb.import.json")
    );
    assert_eq!(ws.index().path_of("as_mesh0001"), Some("models/mesh.glb"));
    assert_eq!(ws.docs().next().expect("valid fixture").0, "models/mesh.glb.import.json");
    let (reference, repair) = refs::resolve("@old.glb#as_mesh0001", None, ws.index()).expect("valid fixture");
    assert_eq!(reference, "@models/mesh.glb#as_mesh0001");
    assert!(repair.is_some());
    assert_eq!(ws.find(&reference).expect("valid fixture").as_str(), "as_mesh0001");

    // The existing API still replaces both paths, including for an existing ID.
    ws.insert(
        "ordinary.scene",
        Document::new("scene", AssetId::parse("as_mesh0001").expect("valid fixture")),
    );
    assert_eq!(ws.path_of("as_mesh0001"), Some("ordinary.scene"));
    assert_eq!(ws.index().path_of("as_mesh0001"), Some("ordinary.scene"));
}
