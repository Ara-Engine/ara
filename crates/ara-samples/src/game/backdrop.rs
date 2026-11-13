//! Derives the native menu vista with ordinary entity and prefab commands.
//!
//! `levels/menu.scene` is a copy of `levels/outpost.scene` without its gameplay: a change made to
//! the level by typed commands has to be made to the menu too, or `ara-samples game` rerun. The
//! test below fails when the two committed scenes disagree.
use super::*;

/// Components the menu leaves out: it shows the compound's art with no gameplay host.
const GAMEPLAY: [&str; 10] = [
    "Survivor",
    "SalvageRun",
    "SalvageCache",
    "SharedDoor",
    "SecurityDrone",
    "ExtractionBeacon",
    "PlayerControl",
    "NavAgent",
    "NavAgentType",
    "NavMeshSurface",
];

/// Whether the menu keeps the level's entity `id`: the extraction beacon and the shared doors
/// exist only for the game.
fn kept(id: &str) -> bool {
    id != "e_gameextract" && !id.starts_with("e_gamedoor")
}

/// The components the menu copies from one level record that is not a prefab instance.
fn art(row: &Value) -> Result<serde_json::Map<String, Value>> {
    Ok(row
        .as_object()
        .context("scene record")?
        .iter()
        .filter(|(key, _)| {
            key.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
                && !GAMEPLAY.contains(&key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

pub(super) fn author(ctx: &Ctx) -> Result<()> {
    const MENU: &str = "levels/menu.scene";
    if ctx.path(MENU).is_file() {
        ctx.apply(
            "ara.project.json",
            "Reauthor the native menu vista",
            vec![(
                "doc.delete",
                json!({"document":"@levels/menu.scene#sc_outpostmenu"}),
            )],
        )?;
    }
    ctx.new_document(MENU, "scene", "sc_outpostmenu")?;
    let source: Value = serde_json::from_slice(&std::fs::read(ctx.path(SCENE))?)?;
    let records = source["records"].as_object().context("outpost records")?;
    let depth = |id: &str| {
        let mut current = id;
        let mut depth = 0;
        while let Some(parent) = records.get(current).and_then(|r| r["parent"].as_str()) {
            depth += 1;
            current = parent;
            if depth > records.len() {
                break;
            }
        }
        depth
    };
    let mut rows: Vec<_> = records.iter().collect();
    rows.sort_by(|(a, av), (b, bv)| {
        depth(a)
            .cmp(&depth(b))
            .then_with(|| av["order"].as_str().cmp(&bv["order"].as_str()))
            .then_with(|| a.cmp(b))
    });
    let mut calls = Vec::new();
    for (id, row) in rows {
        if !kept(id) {
            continue;
        }
        let mut params = json!({"id":id,"name":row["name"],"parent":row.get("parent")});
        if let Some(prefab) = row.get("prefab") {
            params["prefab"] = prefab.clone();
            calls.push(("prefab.instantiate", params));
            for (path, value) in row["overrides"].as_object().into_iter().flatten() {
                calls.push((
                    "prefab.override.set",
                    json!({"entity":id,"path":path,"value":value}),
                ));
            }
        } else {
            params["components"] = art(row)?.into();
            calls.push(("entity.create", params));
        }
    }
    ctx.apply(
        MENU,
        "Compose a native art-only menu backdrop without a gameplay host",
        calls,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(scene: &str) -> serde_json::Map<String, Value> {
        let path = format!(
            "{}/../../samples/outpost/content/levels/{scene}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).expect("a committed Outpost scene");
        let scene: Value = serde_json::from_str(&text).expect("scene JSON");
        scene["records"].as_object().expect("scene records").clone()
    }

    /// Equality that reads `-1` and `-1.0` as the same number: the two scenes can be written by
    /// different commands.
    fn same(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, a)| b.get(key).is_some_and(|b| same(a, b)))
            }
            _ => a == b,
        }
    }

    /// What the menu has to show of one record: its name and parent, and either the prefab with
    /// its overrides or the `components` given.
    fn shown(row: &Value, components: serde_json::Map<String, Value>) -> Value {
        let field = |key: &str| row.get(key).cloned().unwrap_or(Value::Null);
        let mut shown = json!({"name": field("name"), "parent": field("parent")});
        if row.get("prefab").is_some() {
            shown["prefab"] = field("prefab");
            shown["overrides"] = row.get("overrides").cloned().unwrap_or(json!({}));
        } else {
            shown["components"] = components.into();
        }
        shown
    }

    #[test]
    fn the_committed_menu_shows_the_committed_level() {
        let (level, menu) = (records("outpost.scene"), records("menu.scene"));
        let mut wrong: Vec<&str> = menu
            .keys()
            .filter(|id| !kept(id) || !level.contains_key(*id))
            .map(String::as_str)
            .collect();
        for (id, row) in level.iter().filter(|(id, _)| kept(id)) {
            let copied = menu.get(id).is_some_and(|copy| {
                let components = copy
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(key, _)| key.as_bytes().first().is_some_and(u8::is_ascii_uppercase))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                same(
                    &shown(row, art(row).expect("a level record")),
                    &shown(copy, components),
                )
            });
            if !copied {
                wrong.push(id);
            }
        }
        assert!(
            wrong.is_empty(),
            "menu.scene no longer copies outpost.scene in {wrong:?}: repeat the change in the \
             menu, or rerun `ara-samples game`"
        );
    }
}
