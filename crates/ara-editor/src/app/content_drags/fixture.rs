use super::*;

/// Native pointers and file observations use the last semantic index, not a
/// guessed prefix; references and typed authoring inputs keep their logical path.
pub(super) fn storage<'a>(shell: &'a Shell, logical: &str) -> Option<&'a str> {
    shell
        .asset_authored_files()
        .iter()
        .find(|file| {
            matches!(&file.scope, ara_host::project::AuthoredScope::Authored)
                && file.path == logical
        })
        .map(|file| file.storage_path.as_str())
}

/// Build the fixture before opening the host, so verification never mutates the supplied project.
pub(in crate::app) fn fixture(options: &mut Options) -> ara_ui::Result<()> {
    let output = std::path::absolute(&options.output)?;
    let root = output.join(format!("behaviour-project-{}", std::process::id()));
    std::fs::create_dir_all(&output)?;
    // Refuse stale fixtures instead of letting prior imports make a later journey pass.
    std::fs::create_dir(&root)?;
    let content = root.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(content.join("levels"))?;
    std::fs::create_dir_all(content.join("assets"))?;
    std::fs::create_dir_all(output.join("sources"))?;
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Editor behaviours","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
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
        let physical = if path == "ara.project.json" {
            root.join(path)
        } else {
            content.join(path)
        };
        std::fs::write(physical, ara_json::to_canonical_string(&value))?;
    }
    std::fs::write(
        root.join(".ara-behaviour-fixture"),
        "Ara deterministic editor behaviour fixture\n",
    )?;
    let model = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ara-player/demo")
        .join(ara_doc::AUTHORING_ROOT)
        .join("assets/props/rock.glb");
    std::fs::copy(&model, content.join("assets/rock.glb"))
        .map_err(|error| format!("Behaviour model fixture {}: {error}", model.display()))?;
    std::fs::write(
        output.join("sources/os-source.png"),
        include_bytes!("../../../../../assets/brand/icons/engine/ara-engine-64.png"),
    )?;
    options.project = root;
    options.output = output;
    Ok(())
}
