use super::*;
use ara_doc::prefab::Resolved;

/// Seed canonical stress data only before the authority opens the fresh behaviour project.
pub(in crate::app) fn prepare(options: &super::super::Options) -> ara_ui::Result<()> {
    let project = std::path::absolute(&options.project)?;
    let output = std::path::absolute(&options.output)?;
    if !options.verify_behaviours
        || project.parent() != Some(output.as_path())
        || !project
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("behaviour-project-"))
        || !project.join(".ara-behaviour-fixture").is_file()
    {
        return Err("Hierarchy fixture must be inside the fresh isolated behaviour output".into());
    }
    let marker: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join("ara.project.json"))?)?;
    if marker["layoutVersion"] != 3 {
        return Err("Hierarchy fixture expects the freshly created layout-3 authored root".into());
    }
    let path = project
        .join(ara_doc::AUTHORING_ROOT)
        .join("levels/hierarchy-stress.scene");
    ara_doc::AssetId::parse(STRESS).map_err(|error| format!("Invalid stress scene ID: {error}"))?;
    let records: serde_json::Map<String, Value> = (0..COUNT)
        .map(|index| -> ara_ui::Result<(String, Value)> {
            let id = row(index);
            ara_doc::RecordId::parse(&id)
                .map_err(|error| format!("Invalid stress entity ID: {error}"))?;
            Ok((id, json!({"name":format!("Stress row {index:06}")})))
        })
        .collect::<ara_ui::Result<_>>()?;
    let text = ara_json::to_canonical_string(
        &json!({"$schema":"ara://schema/scene/1","id":STRESS,"records":records}),
    );
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

pub(super) fn row(index: usize) -> String {
    format!("e_hstress{index:06}")
}
pub(super) fn command(cmd: &str, args: Value) -> Value {
    json!({"cmd":cmd,"args":args})
}
fn entity(id: &str, name: &str, parent: Option<&str>, transform: Value) -> Value {
    command(
        "entity.create",
        json!({"id":id,"name":name,"parent":parent,"components":{"Transform":transform}}),
    )
}
pub(super) fn create(editor: &mut Editor) -> ara_ui::Result<String> {
    editor.apply(
        vec![command(
            "doc.create",
            json!({"path":"levels/hierarchy-proof.scene","id":SCENE}),
        )],
        "Create native hierarchy fixture scene",
        None,
    )?;
    editor.action("editor.scene.open", json!({"scene":SCENE}))?;
    editor.apply(
        vec![
            entity("e_hsource", "Hierarchy source", None, json!({})),
            entity(
                "e_hsourcechild",
                "Linked child",
                Some("e_hsource"),
                json!({"position":[0,2,0]}),
            ),
            command(
                "prefab.create",
                json!({"entity":"e_hsource","path":"prefabs/hierarchy-proof.prefab","id":PREFAB}),
            ),
        ],
        "Create native hierarchy linked prefab",
        None,
    )?;
    let source = editor
        .host
        .store()
        .workspace()
        .doc(PREFAB)
        .ok_or("Hierarchy prefab was not created")?;
    let root = source
        .children(None)
        .first()
        .map(ToString::to_string)
        .ok_or("Hierarchy prefab has no root")?;
    let child = source
        .children(Some(&root))
        .first()
        .map(ToString::to_string)
        .ok_or("Hierarchy prefab has no child")?;
    Ok(format!("{LINKED}/{child}"))
}

pub(super) fn entities(editor: &Editor) -> ara_ui::Result<Resolved> {
    crate::world::resolve(editor.host.store(), &editor.state.doc)
        .map_err(|diags| format!("{diags:?}").into())
}
pub(super) fn records(editor: &Editor) -> ara_ui::Result<Value> {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Hierarchy scene is missing")?;
    Ok(json!(doc.records()))
}
pub(super) fn setup(
    check: &mut HierarchyCheck,
    editor: &mut Editor,
    shell: &mut Shell,
) -> ara_ui::Result<()> {
    if shell.modal_active() {
        return Err("The previous native journey left a modal active".into());
    }
    shell.leave();
    shell.desktop_pointer = None;
    shell.menu = None;
    editor.action("editor.palette", json!({"open":false}))?;
    editor.action("editor.layout.reset", json!({}))?;
    editor.action(
        "editor.settings.set",
        json!({"key":"input.keymap","value":"ara"}),
    )?;
    editor.action(
        "editor.scene.open",
        json!({"scene":if ACTS[check.index]==Act::Stress {STRESS}else{SCENE}}),
    )?;
    editor.action("editor.select", json!({"entities":[]}))?;
    editor.action("editor.search", json!({"panel":"hierarchy","query":""}))?;
    editor.action("editor.search", json!({"panel":"inspector","query":""}))?;
    editor.action("editor.focus", json!({"panel":"hierarchy"}))?;
    editor.action(
        "editor.camera",
        json!({"target":[0,0,0],"yaw":0.6,"pitch":0.4,"distance":30,"projection":"perspective"}),
    )?;
    if ACTS[check.index] == Act::Stress {
        let loaded = editor
            .host
            .store()
            .workspace()
            .doc(STRESS)
            .ok_or("The canonical stress scene was not loaded")?;
        if loaded.kind != "scene" || loaded.records().len() != COUNT {
            return Err(format!("The loaded stress fixture has kind {} and {} records; expected a scene with {COUNT}",loaded.kind,loaded.records().len()).into());
        }
    }
    if ACTS[check.index] != Act::Stress {
        let roots = editor
            .host
            .store()
            .workspace()
            .doc(SCENE)
            .ok_or("Hierarchy fixture scene missing")?
            .children(None)
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        let mut calls = Vec::new();
        if !roots.is_empty() {
            calls.push(command("entity.delete", json!({"entities":roots})));
        }
        calls.extend([
            entity(COMMON,"Common parent",None,json!({"position":[1,2,3],"rotation":[0,20,0],"scale":[2,2,2]})),
            entity(LEFT,"Left branch",Some(COMMON),json!({"position":[-2,0,0],"rotation":[0,35,0],"scale":[1.5,1.5,1.5]})),
            entity(RIGHT,"Right branch",Some(COMMON),json!({"position":[2,0,0],"rotation":[0,-15,0]})),
            entity(FIRST,"First grouped source",Some(LEFT),json!({"position":[1,1,0],"rotation":[10,0,0]})),
            entity(CHILD,"Selected child",Some(FIRST),json!({"position":[0,2,0]})),
            command("prefab.instantiate",json!({"id":LINKED,"name":"Linked grouped source","prefab":"@prefabs/hierarchy-proof.prefab#pf_hierarchyproof","parent":RIGHT,"position":[1,0,2]})),
            entity(OTHER,"Context outsider",Some(COMMON),json!({"position":[0,3,0]})),
            entity(QUERY_ROOT,"Query world",None,json!({})),
            entity(QUERY_BRANCH,"Lighting branch",Some(QUERY_ROOT),json!({})),
            command("entity.create",json!({"id":QUERY_LEAF,"name":"Key light","parent":QUERY_BRANCH,"components":{"Transform":{},"Light":{}}})),
            command("entity.create",json!({"id":"e_hexcluded","name":"Key light excluded","parent":QUERY_BRANCH,"components":{"Transform":{},"Light":{}}})),
            entity("e_hunrelated","Unrelated mesh",None,json!({})),
        ]);
        editor.apply(calls, "Reset typed native hierarchy fixture", None)?;
        for id in entities(editor)?.keys() {
            editor.action("editor.visibility", json!({"entity":id,"enabled":false}))?;
            editor.action("editor.lock", json!({"entity":id,"enabled":false}))?;
            editor.action("editor.collapse", json!({"entity":id,"enabled":false}))?;
        }
        let selection = match ACTS[check.index] {
            Act::Group => vec![
                FIRST.to_owned(),
                CHILD.to_owned(),
                LINKED.to_owned(),
                check.linked_inner.clone(),
            ],
            Act::Context | Act::Flags => vec![FIRST.into(), LINKED.into()],
            Act::Query | Act::Saved => vec![QUERY_LEAF.into()],
            _ => Vec::new(),
        };
        editor.action("editor.select", json!({"entities":selection}))?;
        if ACTS[check.index] == Act::Query {
            for id in [QUERY_ROOT, QUERY_BRANCH] {
                editor.action("editor.collapse", json!({"entity":id,"enabled":true}))?;
                editor.action("editor.visibility", json!({"entity":id,"enabled":true}))?;
            }
            editor.action(
                "editor.visibility",
                json!({"entity":QUERY_LEAF,"enabled":true}),
            )?;
        }
    }
    shell.hierarchy_offset = 0.;
    shell.rebuild(editor)?;
    shell.input.blur(&mut shell.ui);
    let records = if ACTS[check.index] == Act::Stress {
        Value::Null
    } else {
        records(editor)?
    };
    let world = if ACTS[check.index] == Act::Stress {
        BTreeMap::new()
    } else {
        let entities = entities(editor)?;
        crate::world::world_transforms(&entities)
            .into_iter()
            .map(|(id, matrix)| (id, matrix.to_cols_array()))
            .collect()
    };
    check.before = Baseline {
        revision: editor.host.store().revision(),
        records,
        world,
        camera: editor.state.camera,
        selection: editor.state.selection.clone(),
        collapsed: editor.state.collapsed.clone(),
        hidden: editor.state.hidden.clone(),
        locked: editor.state.locked.clone(),
    };
    Ok(())
}
