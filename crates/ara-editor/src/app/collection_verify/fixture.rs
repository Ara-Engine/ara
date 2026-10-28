//! Typed data setup for native collection gestures; no document files are written here.
use super::*;

pub(super) fn create(editor: &mut Editor) -> ara_ui::Result<()> {
    let list: Vec<_> = (0..COUNT).map(|index| format!("item{index}")).collect();
    let map: serde_json::Map<String, Value> = (0..COUNT)
        .map(|index| (format!("k{index:03}"), json!(index)))
        .collect();
    let layers: Vec<_> = (0..COUNT)
        .map(|index| json!({"id":format!("l_c{index:04}"),"order":index}))
        .collect();
    let other: Vec<_> = (0..COUNT)
        .rev()
        .map(|index| json!({"id":format!("l_c{index:04}"),"order":1000+index}))
        .collect();
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":FIRST,"name":"Collection primary","components":{"Transform":{},"MeshRenderer":{"emissiveMaterials":list},"AudioGraphSource":{"parameters":map},"Tilemap":{"layers":layers}}}}),
        json!({"cmd":"entity.create","args":{"id":SECOND,"name":"Collection unequal","components":{"Transform":{},"MeshRenderer":{"emissiveMaterials":["other0","other1"]},"AudioGraphSource":{"parameters":{"k000":900,"private":42}},"Tilemap":{"layers":other}}}}),
    ], "Create native collection fixtures", None)?;
    Ok(())
}

pub(super) fn records(editor: &Editor) -> ara_ui::Result<Value> {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Collection proof scene disappeared")?;
    Ok(json!({
        (FIRST):doc.record(FIRST).ok_or("Collection primary disappeared")?,
        (SECOND):doc.record(SECOND).ok_or("Collection secondary disappeared")?
    }))
}

pub(super) fn restore(editor: &mut Editor, baseline: &Value) -> ara_ui::Result<()> {
    let mut calls = Vec::new();
    for entity in [FIRST, SECOND] {
        for path in [LIST, KEYED, MAP] {
            let (component, field) = path.split_once('.').ok_or("Invalid fixture field")?;
            calls.push(json!({"cmd":"property.set","args":{"entity":entity,"path":path,"value":baseline[entity][component][field]}}));
        }
    }
    editor.apply(calls, "Reset native collection fixture", None)?;
    Ok(())
}

pub(super) fn expected(act: Act, before: &Value) -> ara_ui::Result<Value> {
    let mut result = before.clone();
    match act {
        Act::ListDrag => {
            let items = result[FIRST]["MeshRenderer"]["emissiveMaterials"]
                .as_array_mut()
                .ok_or("Collection list fixture is not an array")?;
            let item = items.remove(0);
            items.insert(100, item);
        }
        Act::KeyedDrag => {
            for entity in [FIRST, SECOND] {
                let items = result[entity]["Tilemap"]["layers"]
                    .as_array_mut()
                    .ok_or("Collection keyed fixture is not an array")?;
                let from = items
                    .iter()
                    .position(|item| item["id"] == "l_c0000")
                    .ok_or("Collection keyed source missing")?;
                let item = items.remove(from);
                let to = items
                    .iter()
                    .position(|item| item["id"] == "l_c0100")
                    .ok_or("Collection keyed destination missing")?;
                items.insert(to + 1, item);
            }
        }
        Act::ListEdit => result[FIRST]["MeshRenderer"]["emissiveMaterials"][100] = json!(EDIT),
        Act::MapEdit => result[FIRST]["AudioGraphSource"]["parameters"]["k100"] = json!(777),
    }
    Ok(result)
}

pub(super) fn signature(value: &Value) -> Value {
    let bytes = ara_json::to_canonical_string(value);
    json!({"algorithm":"BLAKE3","encoding":"Ara canonical JSON UTF-8","bytes":bytes.len(),"digest":blake3::hash(bytes.as_bytes()).to_hex().to_string()})
}
