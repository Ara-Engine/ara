//! Three-way merge cases.

use ara_diag::codes;
use ara_doc::{Document, read, write};
use ara_merge::merge;
use ara_schema::SchemaDb;

fn db() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    db
}

fn doc(db: &SchemaDb, records: &str) -> Document {
    let text = format!(
        r#"{{ "$schema": "ara://schema/scene/1", "id": "sc_merge001", "records": {{ {records} }} }}"#
    );
    read(&text, "m.scene", db, None)
        .doc
        .expect("test document reads")
}

/// Merges and returns (merged text, conflict paths as `record path`).
fn run(base: &str, ours: &str, theirs: &str) -> (String, Vec<String>) {
    let db = db();
    let merged =
        merge(&doc(&db, base), &doc(&db, ours), &doc(&db, theirs), &db).expect("same document");
    let paths = merged
        .conflicts
        .iter()
        .map(|c| format!("{} {}", c.record, c.path).trim().to_owned())
        .collect();
    (write(&merged.doc, &db), paths)
}

const BASE: &str = r#"
  "e_a": { "name": "A", "order": "a0", "Transform": { "position": [0, 0, 0] } },
  "e_b": { "name": "B", "parent": "e_a", "order": "a0" },
  "e_c": { "name": "C", "order": "a1" }"#;

#[test]
fn disjoint_changes_merge_cleanly() {
    let ours = r#"
      "e_a": { "name": "A", "order": "a0", "Transform": { "position": [1, 0, 0] } },
      "e_b": { "name": "B", "parent": "e_a", "order": "a0" },
      "e_d": { "name": "Ours new", "order": "a2" }"#;
    let theirs = r#"
      "e_a": { "name": "Renamed", "order": "a0", "Transform": { "position": [0, 0, 0], "scale": [2, 2, 2] } },
      "e_b": { "name": "B", "parent": "e_a", "order": "a0" },
      "e_c": { "name": "C", "order": "a1" },
      "e_e": { "name": "Theirs new", "order": "a2" }"#;
    let (text, conflicts) = run(BASE, ours, theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    insta::assert_snapshot!(text);
}

#[test]
fn same_field_changed_differently_conflicts_and_keeps_ours() {
    let ours = BASE.replace(r#""position": [0, 0, 0]"#, r#""position": [1, 0, 0]"#);
    let theirs = BASE
        .replace(r#""position": [0, 0, 0]"#, r#""position": [5, 0, 0]"#)
        .replace(r#""name": "C""#, r#""name": "C2""#);
    let (text, conflicts) = run(BASE, &ours, &theirs);
    assert_eq!(conflicts, ["e_a Transform.position"]);
    insta::assert_snapshot!(text);

    // The output is still a valid document; `ara check` flags the conflict.
    let db = db();
    let reread = read(&text, "m.scene", &db, None);
    let found: Vec<_> = reread.diags.iter().map(|d| d.code).collect();
    assert_eq!(found, [codes::UNRESOLVED_CONFLICT]);
}

#[test]
fn delete_versus_modify_keeps_the_modified_record() {
    let ours = r#"
      "e_a": { "name": "A", "order": "a0", "Transform": { "position": [0, 0, 0] } },
      "e_b": { "name": "B", "parent": "e_a", "order": "a0" }"#;
    let theirs = BASE.replace(r#""name": "C""#, r#""name": "C edited""#);
    let (text, conflicts) = run(BASE, ours, &theirs);
    assert_eq!(conflicts, ["e_c"]);
    assert!(text.contains("C edited"));

    // Deleted on one side and untouched on the other: just deleted.
    let theirs = BASE.to_owned();
    let (text, conflicts) = run(BASE, ours, &theirs);
    assert!(conflicts.is_empty());
    assert!(!text.contains("\"e_c\""));
}

#[test]
fn both_sides_adding_the_same_record() {
    let add = |name: &str| format!(r#"{BASE}, "e_n": {{ "name": "{name}", "order": "a2" }}"#);
    let (_, conflicts) = run(BASE, &add("Same"), &add("Same"));
    assert!(conflicts.is_empty());
    let (_, conflicts) = run(BASE, &add("Ours"), &add("Theirs"));
    assert_eq!(conflicts, ["e_n name"]);
}

#[test]
fn hierarchy_breaks_are_conflicts() {
    // Ours moves A under C, theirs moves C under A: a loop.
    let ours = BASE.replace(
        r#""name": "A", "order": "a0""#,
        r#""name": "A", "parent": "e_c", "order": "a0""#,
    );
    let theirs = BASE.replace(
        r#""name": "C", "order": "a1""#,
        r#""name": "C", "parent": "e_a", "order": "a1""#,
    );
    let (_, conflicts) = run(BASE, &ours, &theirs);
    assert_eq!(conflicts, ["e_a parent", "e_c parent"]);

    // Ours deletes C, theirs adds a child under it.
    let ours = r#"
      "e_a": { "name": "A", "order": "a0", "Transform": { "position": [0, 0, 0] } },
      "e_b": { "name": "B", "parent": "e_a", "order": "a0" }"#;
    let theirs = format!(r#"{BASE}, "e_k": {{ "name": "Kid", "parent": "e_c", "order": "a0" }}"#);
    let (_, conflicts) = run(BASE, ours, &theirs);
    assert_eq!(conflicts, ["e_k parent"]);
}

#[test]
fn override_maps_merge_per_key() {
    let base = r#""e_i": { "prefab": "@prefabs/p.prefab#pf_p", "overrides": { "name": "I" } }"#;
    let ours = r#""e_i": { "prefab": "@prefabs/p.prefab#pf_p", "overrides": { "name": "I", "e_x/Transform.position": [1, 0, 0] } }"#;
    let theirs = r#""e_i": { "prefab": "@prefabs/p.prefab#pf_p", "overrides": { "name": "I", "e_y/Light": {} } }"#;
    let (text, conflicts) = run(base, ours, theirs);
    assert!(conflicts.is_empty());
    assert!(text.contains("e_x/Transform.position") && text.contains("e_y/Light"));
}
