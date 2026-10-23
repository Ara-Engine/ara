use anyhow::{Result, ensure};
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;
#[test]
fn generated_tile_hulls_are_owned_replaceable_and_undoable() -> Result<()> {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut commands = Registry::builtin(&mut db);
    ara_2d::commands::register(&mut commands, &mut db);
    ara_physics2d::commands::register(&mut commands, &mut db);
    ensure!(
        db.verify().is_empty() && commands.check_examples(&db).is_empty(),
        "invalid schema/command examples"
    );
    let mut workspace = Workspace::default();
    workspace.insert(
        "map.scene",
        Document::new(
            "scene",
            AssetId::parse("sc_collide1").map_err(anyhow::Error::msg)?,
        ),
    );
    let mut store = Store::new(db, workspace);
    let session = Session::human();
    let setup = Batch::new(
        "sc_collide1",
        [
            ("entity.create", json!({"id":"e_map1"})),
            (
                "tilemap.set",
                json!({"entity":"e_map1","value":{"definitions":{"1":{"solid":true}},"layers":[{"id":"l_ground","cells":{"0,0":{"tile":1},"1,0":{"tile":1}}}]}}),
            ),
        ],
    );
    commands
        .apply(&mut store, &setup, &session)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let before = ara_doc::write(
        store
            .workspace()
            .doc("sc_collide1")
            .expect("loaded document"),
        store.schema(),
    );
    let generate = Batch::new(
        "sc_collide1",
        [(
            "tilemap.colliders.generate",
            json!({"entity":"e_map1","origin":[0,0]}),
        )],
    );
    commands
        .apply(&mut store, &generate, &session)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(
        store
            .workspace()
            .doc("sc_collide1")
            .expect("loaded document")
            .records()
            .len()
            == 2,
        "adjacent grid tiles were not merged"
    );
    commands
        .apply(&mut store, &generate, &session)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(
        store
            .workspace()
            .doc("sc_collide1")
            .expect("loaded document")
            .records()
            .len()
            == 2,
        "generation leaked old owned entities"
    );
    store
        .undo_last(&Author::Human)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    store
        .undo_last(&Author::Human)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(
        before
            == ara_doc::write(
                store
                    .workspace()
                    .doc("sc_collide1")
                    .expect("loaded document"),
                store.schema()
            ),
        "collider undo did not restore source"
    );
    Ok(())
}
#[test]
fn physics_set_swaps_roles_atomically_and_rejects_mixed_roles() -> Result<()> {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut commands = Registry::builtin(&mut db);
    ara_physics2d::commands::register(&mut commands, &mut db);
    let mut workspace = Workspace::default();
    workspace.insert(
        "body.scene",
        Document::new(
            "scene",
            AssetId::parse("sc_body0001").map_err(anyhow::Error::msg)?,
        ),
    );
    let mut store = Store::new(db, workspace);
    let session = Session::human();
    let apply = |store: &mut Store, command: &'static str, params: serde_json::Value| {
        commands
            .apply(
                store,
                &Batch::new("sc_body0001", [(command, params)]),
                &session,
            )
            .map(drop)
    };
    apply(&mut store, "entity.create", json!({"id":"e_body1"}))
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    apply(&mut store, "physics2d.set", json!({"entity":"e_body1","pose":{"position":[1,2]},"body":{"motion":"dynamic"},"collider":{}}))
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    apply(
        &mut store,
        "physics2d.set",
        json!({"entity":"e_body1","pose":{"position":[1,2]},"character":{}}),
    )
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let record = |store: &Store| {
        store
            .workspace()
            .doc("sc_body0001")
            .expect("loaded document")
            .records()
            .values()
            .next()
            .cloned()
            .expect("entity")
    };
    let character = record(&store);
    ensure!(
        character.get("CharacterController2d").is_some()
            && character.get("RigidBody2d").is_none()
            && character.get("Collider2d").is_none(),
        "role swap left stale components"
    );
    ensure!(
        apply(
            &mut store,
            "physics2d.set",
            json!({"entity":"e_body1","pose":{},"body":{},"collider":{},"character":{}})
        )
        .is_err()
            && apply(
                &mut store,
                "physics2d.set",
                json!({"entity":"e_body1","pose":{},"character":{"height":0.2}})
            )
            .is_err(),
        "mixed roles or invalid capsules must be rejected"
    );
    store
        .undo_last(&Author::Human)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(
        record(&store).get("RigidBody2d").is_some(),
        "undo restores the body role"
    );
    Ok(())
}
