//! Derives the native menu vista with ordinary entity and prefab commands.
use super::*;

pub(super) fn author(ctx: &Ctx) -> Result<()> {
    const MENU: &str = "levels/menu.scene";
    if ctx.root.join(MENU).is_file() {
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
    let source: Value = serde_json::from_slice(&std::fs::read(ctx.root.join(SCENE))?)?;
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
    let excluded = [
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
    let mut calls = Vec::new();
    for (id, row) in rows {
        if id == "e_gameextract" || id.starts_with("e_gamedoor") {
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
            let components: serde_json::Map<_, _> = row
                .as_object()
                .context("scene record")?
                .iter()
                .filter(|(key, _)| {
                    key.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
                        && !excluded.contains(&key.as_str())
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            params["components"] = components.into();
            calls.push(("entity.create", params));
        }
    }
    ctx.apply(
        MENU,
        "Compose a native art-only menu backdrop without a gameplay host",
        calls,
    )
}
