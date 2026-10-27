use super::*;

/// Build the fixture before opening the host, so verification never mutates the supplied project.
pub(in crate::app) fn fixture(options: &mut Options) -> ara_ui::Result<()> {
    let output = std::path::absolute(&options.output)?;
    let root = output.join(format!("behaviour-project-{}", std::process::id()));
    std::fs::create_dir_all(&output)?;
    // Refuse stale fixtures instead of letting prior imports make a later journey pass.
    std::fs::create_dir(&root)?;
    std::fs::create_dir_all(root.join("levels"))?;
    std::fs::create_dir_all(root.join("assets"))?;
    std::fs::create_dir_all(output.join("sources"))?;
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Editor behaviours","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_behave01","records":{}}),
        ),
        (
            "assets/box.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_behave01","records":{"e_prefab01":{"name":"Box","Transform":{},"MeshRenderer":{}}}}),
        ),
    ] {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value))?;
    }
    std::fs::write(
        root.join(".ara-behaviour-fixture"),
        "Ara deterministic editor behaviour fixture\n",
    )?;
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ara-player/demo/assets/props/rock.glb"
        ),
        root.join("assets/rock.glb"),
    )?;
    std::fs::write(
        output.join("sources/os-source.png"),
        include_bytes!("../../../../../assets/brand/icons/engine/ara-engine-64.png"),
    )?;
    options.project = root;
    options.output = output;
    Ok(())
}
