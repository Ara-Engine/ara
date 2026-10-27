//! Source sidecars must not become dirty merely by opening a sample project.
use ara_asset::{Sidecar, SidecarCommand};
use std::path::{Path, PathBuf};

fn sidecars(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            sidecars(&entry.path(), files)?;
        } else if entry
            .file_name()
            .to_string_lossy()
            .ends_with(".import.json")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

#[test]
fn sample_sidecars_match_installed_importer_versions() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
    let registry = ara_importers::registry()?;
    let installed = registry.versions();
    let update = std::env::var_os("ARA_UPDATE_DEMO").is_some();
    let mut stale = Vec::new();
    for project in ["outpost", "showcase"] {
        let project_root = root.join(project);
        let mut files = Vec::new();
        sidecars(&project_root.join(ara_doc::AUTHORING_ROOT), &mut files)?;
        files.sort();
        assert!(!files.is_empty(), "{project}: missing sample sidecars");
        let mut db = ara_schema::SchemaDb::new();
        ara_doc::register(&mut db);
        ara_asset::register_schema(&mut db);
        let mut workspace = ara_doc::Workspace::default();
        let mut changes = Vec::new();
        for path in files {
            let relative = path
                .strip_prefix(&project_root)?
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path)?;
            let sidecar = Sidecar::read(&text, &relative, &db)?;
            let settings = sidecar.settings()?;
            let current = installed.get(&settings.importer).ok_or_else(|| {
                format!(
                    "{project}/{relative}: unknown sample importer {}",
                    settings.importer
                )
            })?;
            if settings.version != *current {
                stale.push(format!(
                    "{project}/{relative}: {} -> {current}",
                    settings.version
                ));
                changes.push((path, sidecar.document().id.clone(), current.clone()));
            }
            workspace.insert(relative, sidecar.into_document());
        }
        if update && !changes.is_empty() {
            let mut store = ara_ops::Store::new(db, workspace);
            let edits = changes
                .iter()
                .map(|(_, id, version)| SidecarCommand::Version(version.clone()).edit(id.clone()))
                .collect::<ara_asset::Result<Vec<_>>>()?;
            store
                .transact(
                    ara_ops::TxnMeta::by(ara_ops::Author::Agent("samples".into())),
                    edits,
                )
                .expect("registry versions are valid edits to loaded sample sidecars");
            for (path, id, _) in changes {
                let doc = store
                    .workspace()
                    .doc(id.as_str())
                    .expect("loaded sample sidecar");
                std::fs::write(path, ara_doc::write(doc, store.schema()))?;
            }
        }
    }
    assert!(
        update || stale.is_empty(),
        "sample importer versions are stale:\n{}\nRegenerate with ARA_UPDATE_DEMO=1 cargo test -p ara-importers --test sample_versions",
        stale.join("\n")
    );
    Ok(())
}
