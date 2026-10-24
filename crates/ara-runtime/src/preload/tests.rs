#![allow(clippy::unwrap_used)]
use super::*;
use ara_doc::{Document, RecordId};
use serde_json::json;

fn add(workspace: &mut Workspace, kind: &str, id: &str, records: Value) {
    let mut document = Document::new(kind, AssetId::parse(id).unwrap());
    for (key, value) in records.as_object().unwrap() {
        document.insert_unchecked(
            RecordId::parse(key).unwrap(),
            serde_json::from_value(value.clone()).unwrap(),
        );
    }
    workspace.insert(format!("{id}.{kind}"), document);
}

#[test]
fn preload_follows_prefabs_cells_and_foliage_without_loading_other_scenes() {
    let mut workspace = Workspace::default();
    add(
        &mut workspace,
        "scene",
        "sc_main",
        json!({
            "e_root":{"MeshRenderer":{"model":"@main.glb#as_main"}},
            "e_part":{"prefab":"@pf_outer.prefab#pf_outer"},
            "e_world":{"StreamedWorld":{"world":"@wd_main.world#wd_main"}},
            "e_ground":{"Terrain":{"foliage":"@fo_scatter.foliage#fo_scatter"}}
        }),
    );
    add(
        &mut workspace,
        "prefab",
        "pf_outer",
        json!({"e_nested":{"prefab":"@pf_inner.prefab#pf_inner"}}),
    );
    add(
        &mut workspace,
        "prefab",
        "pf_inner",
        json!({"e_mesh":{"MeshRenderer":{"model":"@inner.glb#as_inner"}}}),
    );
    add(
        &mut workspace,
        "world",
        "wd_main",
        json!({"c_far":{"documents":["@sc_far.scene#sc_far"],"actors":["@sc_actor.scene#sc_actor"]}}),
    );
    add(
        &mut workspace,
        "scene",
        "sc_far",
        json!({"e_part":{"prefab":"@pf_cell.prefab#pf_cell"}}),
    );
    add(
        &mut workspace,
        "prefab",
        "pf_cell",
        json!({"e_model":{"MeshRenderer":{"model":"@cell.glb#as_cell"}}}),
    );
    add(
        &mut workspace,
        "scene",
        "sc_actor",
        json!({"e_model":{"Collider":{"kind":"modelConvex","asset":"@collider.glb#as_collider"}}}),
    );
    add(
        &mut workspace,
        "foliage",
        "fo_scatter",
        json!({
            "r_tree":{"asset":"@pf_tree.prefab#pf_tree"},
            "r_rock":{"asset":"@rock.glb#as_rock","enabled":false}
        }),
    );
    add(
        &mut workspace,
        "prefab",
        "pf_tree",
        json!({"e_tree":{"MeshRenderer":{"model":"@tree.glb#as_tree"}}}),
    );
    add(
        &mut workspace,
        "scene",
        "sc_other",
        json!({"e_model":{"MeshRenderer":{"model":"@unused.glb#as_unused"}}}),
    );
    add(
        &mut workspace,
        "prefab",
        "pf_other",
        json!({"e_model":{"MeshRenderer":{"model":"@unused.glb#as_unused"}}}),
    );
    let expected = [
        "as_main",
        "as_inner",
        "as_cell",
        "as_collider",
        "as_tree",
        "as_rock",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        models(&workspace, &AssetId::parse("sc_main").unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        models(&workspace, &AssetId::parse("sc_other").unwrap()).unwrap(),
        BTreeSet::from(["as_unused".to_owned()])
    );
    let mut live = BTreeSet::new();
    extend(&workspace, &json!("@pf_other.prefab#pf_other"), &mut live).unwrap();
    assert_eq!(live, BTreeSet::from(["as_unused".to_owned()]));
}

#[test]
fn record_references_use_their_document_path_and_cycles_terminate() {
    let mut workspace = Workspace::default();
    add(
        &mut workspace,
        "scene",
        "sc_main",
        json!({"e_link":{"target":"@sc_child.scene#e_child"}}),
    );
    add(
        &mut workspace,
        "scene",
        "sc_child",
        json!({"e_child":{"target":"@sc_main.scene#e_link","MeshRenderer":{"model":"@mesh.glb#as_mesh"}}}),
    );
    assert_eq!(
        models(&workspace, &AssetId::parse("sc_main").unwrap()).unwrap(),
        BTreeSet::from(["as_mesh".to_owned()])
    );
}
