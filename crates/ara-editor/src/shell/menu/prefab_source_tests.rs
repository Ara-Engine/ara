//! The actual Apply To menu emits prefab asset references and preserves context intent.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn open() -> ara_ui::Result<(Editor, Shell)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::test_target()
        .join("prefab-source-menus")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    let content = root.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(content.join("levels"))?;
    std::fs::create_dir_all(content.join("prefabs"))?;
    let marker = json!({"name":"Prefab source menu","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION});
    std::fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(&marker),
    )?;
    for (path, value) in [
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_menuprefab","records":{
                "e_instance":{"name":"Outer instance","prefab":"@prefabs/outer.prefab#pf_menuouter"},
                "e_unrelated":{"name":"Unrelated selection","Transform":{}}
            }}),
        ),
        (
            "prefabs/base.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_menubase","records":{
                "e_baseroot":{"name":"Base","Transform":{"scale":[1,1,1]},"MeshRenderer":{"size":[1,1,1]}}
            }}),
        ),
        (
            "prefabs/outer.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_menuouter","records":{
                "e_outerroot":{"name":"Outer","Transform":{}},
                "e_nestedbase":{"name":"Nested base","parent":"e_outerroot","prefab":"@prefabs/base.prefab#pf_menubase"}
            }}),
        ),
    ] {
        std::fs::write(content.join(path), ara_json::to_canonical_string(&value))?;
    }
    let mut editor = Editor::open(&root)?;
    editor.apply(vec![
        json!({"cmd":"property.set","args":{"entity":"e_instance/e_nestedbase","path":"Transform.scale","value":[3,4,5]}}),
        json!({"cmd":"property.set","args":{"entity":"e_instance/e_nestedbase","path":"MeshRenderer.size","value":[2,2,2]}}),
    ],"Prepare nested prefab menu overrides",None)?;
    let shell = Shell::new(&mut editor, [1100., 750.])?;
    Ok((editor, shell))
}

fn snapshot(editor: &Editor) -> std::collections::BTreeMap<String, String> {
    editor
        .host
        .store()
        .workspace()
        .docs()
        .map(|(path, doc)| {
            (
                path.to_owned(),
                ara_doc::write(doc, editor.host.store().schema()),
            )
        })
        .collect()
}

#[test]
fn legacy_projects_are_rejected_without_mutating_prefab_menu_fixtures() -> ara_ui::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = crate::test_target()
        .join("prefab-menu-legacy-rejection")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    for version in [0, 3] {
        let root = base.join(version.to_string());
        std::fs::create_dir_all(root.join("levels"))?;
        let marker = if version == 0 {
            json!({"name":"Legacy menu input","main":"levels/main.scene"})
        } else {
            json!({"name":"Preview3 menu input","main":"levels/main.scene","layoutVersion":3})
        };
        let marker_bytes = ara_json::to_canonical_string(&marker).into_bytes();
        let scene_bytes = b"original legacy menu scene bytes";
        std::fs::write(root.join("ara.project.json"), &marker_bytes)?;
        std::fs::write(root.join("levels/main.scene"), scene_bytes)?;
        let error = Editor::open_with(&root, Some(root.join(".ara/test-user.json")))
            .err()
            .ok_or("A legacy menu fixture was opened without migration")?;
        assert!(error.to_string().contains("migrate"), "{error}");
        assert_eq!(std::fs::read(root.join("ara.project.json"))?, marker_bytes);
        assert_eq!(std::fs::read(root.join("levels/main.scene"))?, scene_bytes);
        assert_eq!(
            std::fs::read_dir(&root)?.count(),
            2,
            "Rejected open must not add content, cache or profile state"
        );
        assert_eq!(std::fs::read_dir(root.join("levels"))?.count(), 1);
    }
    Ok(())
}
fn field(editor: &Editor, path: &str) -> Option<Vec<f64>> {
    editor
        .host
        .store()
        .workspace()
        .doc("pf_menubase")?
        .record("e_baseroot")?
        .get(path.split_once('.')?.0)?
        .get(path.split_once('.')?.1)?
        .as_array()?
        .iter()
        .map(Value::as_f64)
        .collect()
}

#[test]
fn apply_to_menu_uses_logical_asset_ids_and_preserves_nested_context_and_field_scope()
-> ara_ui::Result<()> {
    for field_only in [false, true] {
        let (mut editor, mut shell) = open()?;
        let entity = "e_instance/e_nestedbase";
        editor.action(
            "editor.select",
            json!({"entities":[if field_only {entity}else {"e_unrelated"}]}),
        )?;
        let target = if field_only {
            Target::Field("Transform.scale".into())
        } else {
            Target::Entity(entity.into())
        };
        shell.open_menu_at(
            Source::Context(
                if field_only {
                    Context::Field
                } else {
                    Context::Hierarchy
                },
                target,
            ),
            [100., 100.],
        );
        let mut rows = Vec::new();
        shell.dynamic(&editor, Dynamic::PrefabSources, &mut rows);
        assert_eq!(
            rows.len(),
            2,
            "Nested instance must offer outer and base assets"
        );
        let mut chosen = None;
        for row in rows {
            let Row::Command {
                id,
                target: Target::Text(text),
                label,
                ..
            } = row
            else {
                return Err("Source menu emitted a non-command row".into());
            };
            let choice: Value = serde_json::from_str(&text)?;
            let reference = choice["destination"]
                .as_str()
                .ok_or("Source menu omitted destination")?;
            let parts = ara_schema::grammar::parse_ref(reference)
                .map_err(|error| format!("Invalid menu reference: {error}"))?;
            assert!(
                parts.id.is_some_and(|id| id.starts_with("pf_")),
                "Apply To requires an asset identity, not an entity record: {reference}"
            );
            assert_eq!(choice["entity"], entity);
            assert_eq!(
                choice["path"],
                if field_only {
                    json!("Transform.scale")
                } else {
                    Value::Null
                }
            );
            assert_eq!(label.as_deref(), parts.path);
            if reference == "@prefabs/base.prefab#pf_menubase" {
                chosen = Some((id, Target::Text(text)));
            }
        }
        let (command, target) =
            chosen.ok_or("Native J5's logical base asset reference is absent from Apply To")?;
        let before = snapshot(&editor);
        let revision = editor.host.store().revision();
        shell.run_command(&mut editor, &command, &target)?;
        assert_eq!(
            editor.host.store().revision(),
            revision + 1,
            "Apply To must be one authoring transaction"
        );
        assert_eq!(field(&editor, "Transform.scale"), Some(vec![3., 4., 5.]));
        assert_eq!(
            field(&editor, "MeshRenderer.size"),
            Some(if field_only {
                vec![1., 1., 1.]
            } else {
                vec![2., 2., 2.]
            })
        );
        let touched: std::collections::BTreeSet<_> = editor
            .host
            .store()
            .since(revision)
            .iter()
            .flat_map(|txn| txn.ops.iter().map(|op| op.doc().as_str()))
            .collect();
        assert_eq!(
            touched,
            std::collections::BTreeSet::from(["pf_menubase", "sc_menuprefab"])
        );
        assert_eq!(
            editor.state.selection,
            vec![(if field_only { entity } else { "e_unrelated" }).to_owned()]
        );
        editor.action("history.undo", json!({}))?;
        assert_eq!(
            snapshot(&editor),
            before,
            "One undo must restore source and nested instance together"
        );
    }

    Ok(())
}

#[test]
fn linked_root_can_create_a_variant_in_one_undo_but_resolved_children_stay_blocked()
-> ara_ui::Result<()> {
    let (mut editor, mut shell) = open()?;
    let command = commands::get("create.prefab").ok_or("Missing prefab command")?;
    let child = Target::Entity("e_instance/e_nestedbase".into());
    assert!(matches!(
        command.avail(&shell.ctx(&editor, &child)),
        Avail::Blocked(_)
    ));
    let root = Target::Entity("e_instance".into());
    assert!(
        command.avail(&shell.ctx(&editor, &root)).ready(),
        "A linked root must expose the variant workflow its handler supports"
    );
    let before = snapshot(&editor);
    let revision = editor.host.store().revision();
    shell.run_command(&mut editor, command.id, &root)?;
    assert_eq!(editor.host.store().revision(), revision + 1);
    let workspace = editor.host.store().workspace();
    let reference = workspace
        .doc("sc_menuprefab")
        .and_then(|doc| doc.record("e_instance"))
        .and_then(|record| record.get("prefab"))
        .and_then(Value::as_str)
        .ok_or("Created variant instance has no source")?;
    assert_ne!(reference, "@prefabs/outer.prefab#pf_menuouter");
    let variant = workspace
        .find(reference)
        .and_then(|id| workspace.doc(id.as_str()))
        .ok_or("Created variant document is missing")?;
    let variant_root = variant
        .children(None)
        .first()
        .and_then(|id| variant.record(id.as_str()));
    assert_eq!(
        variant_root
            .and_then(|record| record.get("prefab"))
            .and_then(Value::as_str),
        Some("@prefabs/outer.prefab#pf_menuouter")
    );
    editor.action("history.undo", json!({}))?;
    assert_eq!(
        snapshot(&editor),
        before,
        "One Undo must restore the original instance and remove its new variant"
    );

    Ok(())
}
