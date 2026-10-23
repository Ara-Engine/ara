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
