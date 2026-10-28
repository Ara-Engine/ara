//! The optional root preferences document has one exact lifecycle exception.
use ara_doc::{AssetId, Edit, Op, Workspace};
use ara_schema::{DocKindDesc, SchemaDb};

fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    db.add_kind(DocKindDesc {
        name: "project".into(),
        version: 1,
        doc: "Project metadata and policy.".into(),
        id_prefix: "pj".into(),
        extension: "project.json".into(),
        records: Vec::new(),
        single_root: false,
    })
    .expect("The fixture adds a distinct project kind");
    db
}

#[test]
fn optional_root_preferences_create_and_inverse_keep_their_project_storage_identity() {
    let db = schema();
    let mut workspace = Workspace::default();
    workspace.set_reference_root("content");
    let id = AssetId::parse("pj_preferences").expect("The reserved preferences ID is valid");
    let op = workspace
        .apply(
            Edit::Document {
                doc: id.clone(),
                path: "settings/project.json".into(),
                kind: Some("project".into()),
            },
            &db,
        )
        .expect("The exact optional root preferences document is authorable");
    assert!(
        matches!(&op, Op::Document { doc, path, kind: Some(kind), prev: None }
        if doc == &id && path == "settings/project.json" && kind == "project")
    );
    assert_eq!(
        workspace.path_of(id.as_str()),
        Some("settings/project.json")
    );
    assert!(workspace.is_project_document(id.as_str()));
    assert!(workspace.index().path_of(id.as_str()).is_none());
    workspace.revert(&op, &db);
    assert!(workspace.doc(id.as_str()).is_none());
    assert!(workspace.docs().next().is_none());
}

#[test]
fn other_absent_documents_cannot_use_the_preferences_lifecycle_exception() {
    let db = schema();
    for (kind, id, path) in [
        ("project", "pj_otherprefs", "settings/project.json"),
        ("project", "pj_preferences", "content/settings/project.json"),
        ("project", "pj_preferences", "settings/Project.json"),
        ("project", "pj_preferences", "settings/other.json"),
        ("project", "sc_preferences", "settings/project.json"),
        ("scene", "pj_preferences", "settings/project.json"),
    ] {
        let mut workspace = Workspace::default();
        workspace.set_reference_root("content");
        let generation = workspace.generation();
        assert!(
            workspace
                .apply(
                    Edit::Document {
                        doc: AssetId::parse(id)
                            .expect("The near-miss fixture ID is syntactically valid"),
                        path: path.into(),
                        kind: Some(kind.into()),
                    },
                    &db
                )
                .is_err(),
            "Unexpected lifecycle exception for {kind}/{id}/{path}"
        );
        assert_eq!(workspace.generation(), generation);
        assert!(workspace.docs().next().is_none());
    }
}
