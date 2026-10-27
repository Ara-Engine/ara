//! Build profiles use the same prepared, reversible file transitions as source edits.
use crate::{Command, EditCtx};
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::{FileKind, FileState};
use ara_schema::Schema;
use serde::Deserialize;
use serde_json::{Value, json};

/// Owner key for exact canonical profile bytes prepared by a file worker.
pub const BUILD_PROFILE_METADATA: &str = "ara.buildProfile";

/// Parameters of build_profile.create.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildProfileCreateParams {
    /// Portable profile name; its filename is normalized to lowercase.
    pub name: String,
    /// Complete export preset, written with the canonical JSON writer.
    pub preset: Value,
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message)
}

/// Resolve a portable profile name to its fixed project-relative settings path.
pub fn build_profile_path(name: &str) -> Result<String, Diagnostic> {
    let name = name.to_ascii_lowercase();
    let reserved = matches!(name.as_str(), "con" | "prn" | "aux" | "nul")
        || ["com", "lpt"].into_iter().any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
            })
        });
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || reserved
    {
        return Err(invalid(
            "build profile names use 1–64 ASCII letters, digits, hyphens or underscores and cannot be reserved Windows names",
        ));
    }
    Ok(format!("settings/builds/{name}.export.json"))
}

/// Exact bytes shared by the pure command and its snapshot-preparation worker.
pub fn build_profile_bytes(preset: &Value) -> Result<Vec<u8>, Diagnostic> {
    if !preset.is_object() {
        return Err(invalid("a build profile preset must be a JSON object"));
    }
    Ok(ara_json::to_canonical_string(&ara_json::sort_keys(preset)).into_bytes())
}

/// Create a checked export preset through attributed file ops, including new parents.
pub struct BuildProfileCreate;

impl Command for BuildProfileCreate {
    type Params = BuildProfileCreateParams;
    type Output = Value;
    const NAME: &'static str = "build_profile.create";
    const SUMMARY: &'static str = "Create a portable export profile with reversible file history.";
    const SCOPE: &'static str = "edit:assets";

    fn examples() -> Vec<Value> {
        vec![
            json!({"name":"windows","preset":{"platform":"windows","config":"dev","lane":"nativeAot","playerTemplate":"runtime","assembly":"Game","content":ara_doc::AUTHORING_ROOT,"templates":[]}}),
        ]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        let path = build_profile_path(&params.name)?;
        let state = FileState::bytes(&build_profile_bytes(&params.preset)?);
        let name = params.name.to_ascii_lowercase();
        let metadata = ctx
            .prepared_metadata()
            .and_then(|metadata| metadata.get(BUILD_PROFILE_METADATA))
            .and_then(Value::as_array)
            .and_then(|profiles| {
                profiles.iter().find(|profile| {
                    profile["path"] == path
                        && profile["name"] == name
                        && profile["preset"] == params.preset
                })
            })
            .ok_or_else(|| {
                invalid("build profile creation needs matching worker-prepared profile metadata")
            })?;
        let prepared_state: FileState = serde_json::from_value(metadata["state"].clone())
            .map_err(|error| invalid(format!("invalid prepared build profile state: {error}")))?;
        if prepared_state != state {
            return Err(Diagnostic::error(
                codes::STALE_BASE,
                "prepared build profile bytes do not match the requested canonical preset",
            )
            .file(&path));
        }
        let read = ctx.read();
        let prepared = read
            .prepared_files()
            .ok_or_else(|| invalid("build profile creation needs a prepared file inventory"))?;
        for entry in ["settings", "settings/builds", path.as_str()] {
            if !prepared.contains(entry) {
                return Err(invalid(format!(
                    "build profile path `{entry}` is outside the checked prepared inventory"
                )));
            }
        }
        for parent in ["settings", "settings/builds"] {
            if ctx
                .file_state(parent)
                .is_some_and(|state| state.kind != FileKind::Directory)
            {
                return Err(invalid(format!(
                    "build profile parent `{parent}` must be a directory"
                ))
                .file(parent));
            }
        }
        if ctx.file_state(&path).is_some() {
            return Err(
                invalid("the build profile already exists; choose another name").file(&path),
            );
        }
        for parent in ["settings", "settings/builds"] {
            if ctx.file_state(parent).is_none() {
                ctx.set_file(parent, Some(FileState::directory()))?;
            }
        }
        ctx.set_file(&path, Some(state.clone()))?;
        Ok(json!({"name":name,"path":path,"hash":state.hash}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Batch, Registry, Session};
    use ara_doc::{AssetId, Document, Workspace, file_state::PreparedInventory};
    use ara_ops::Store;
    use ara_schema::SchemaDb;

    fn fixture(preset: &Value) -> (Registry, Store, PreparedInventory) {
        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        let mut registry = Registry::new();
        registry.command::<BuildProfileCreate>(&mut db);
        let mut workspace = Workspace::default();
        workspace.insert(
            "main.scene",
            Document::new("scene", AssetId::parse("sc_profile").expect("scene")),
        );
        let path = build_profile_path("Windows").expect("path");
        let state = FileState::bytes(&build_profile_bytes(preset).expect("canonical bytes"));
        let inventory = PreparedInventory {
            entries: ["settings", "settings/builds", path.as_str()]
                .into_iter()
                .map(|path| (path.to_owned(), None))
                .collect(),
            metadata: json!({BUILD_PROFILE_METADATA:[{"name":"windows","path":path,"preset":preset,"state":state}]}),
            ..PreparedInventory::default()
        };
        (registry, Store::new(db, workspace), inventory)
    }

    #[test]
    fn prepared_profile_creation_dry_runs_and_undoes_directory_and_file_ops() {
        let preset = json!({"platform":"windows","assembly":"Example","content":"game"});
        let (registry, mut store, inventory) = fixture(&preset);
        store.prepare_files(inventory).expect("prepared inventory");
        let mut batch = Batch::new(
            "sc_profile",
            [(
                BuildProfileCreate::NAME,
                json!({"name":"Windows","preset":preset}),
            )],
        );
        batch.dry_run = true;
        let preview = registry
            .apply(&mut store, &batch, &Session::human())
            .expect("dry run");
        assert_eq!(preview.ops.len(), 3);
        assert_eq!(store.revision(), 0);
        assert!(store.workspace().file_state("settings").is_none());
        batch.dry_run = false;
        let result = registry
            .apply(&mut store, &batch, &Session::human())
            .expect("create");
        assert_eq!(preview.ops, result.ops);
        let path = "settings/builds/windows.export.json";
        assert_eq!(
            store.workspace().file_state(path),
            Some(&FileState::bytes(
                &build_profile_bytes(&preset).expect("bytes")
            ))
        );
        assert_eq!(result.outputs[0]["name"], "windows");
        store.undo_last(&Session::human().author).expect("undo");
        for path in ["settings", "settings/builds", path] {
            assert!(store.workspace().file_state(path).is_none());
        }
    }

    #[test]
    fn profile_creation_rejects_stale_metadata_and_existing_targets_atomically() {
        let preset = json!({"platform":"windows"});
        let (registry, mut store, mut inventory) = fixture(&preset);
        inventory.metadata[BUILD_PROFILE_METADATA][0]["state"] =
            serde_json::to_value(FileState::bytes(b"wrong bytes")).expect("state");
        store.prepare_files(inventory).expect("inventory");
        let batch = Batch::new(
            "sc_profile",
            [(
                BuildProfileCreate::NAME,
                json!({"name":"windows","preset":preset}),
            )],
        );
        assert!(
            registry
                .apply(&mut store, &batch, &Session::human())
                .is_err()
        );
        assert_eq!(store.revision(), 0);
        assert!(store.workspace().file_state("settings").is_none());
        let (registry, mut store, mut inventory) = fixture(&preset);
        let path = "settings/builds/windows.export.json";
        let previous = FileState::bytes(b"existing");
        inventory
            .entries
            .insert(path.into(), Some(previous.clone()));
        store.prepare_files(inventory).expect("inventory");
        assert!(
            registry
                .apply(&mut store, &batch, &Session::human())
                .is_err()
        );
        assert_eq!(store.revision(), 0);
        assert_eq!(store.workspace().file_state(path), Some(&previous));
    }

    #[test]
    fn profile_names_are_portable_and_presets_are_objects() {
        assert_eq!(
            build_profile_path("Windows_Desktop-1").expect("portable"),
            "settings/builds/windows_desktop-1.export.json"
        );
        for name in [
            "",
            "../outside",
            "a/b",
            "a\\b",
            "name.json",
            "name ",
            "con",
            "COM1",
            "lpt9",
            "é",
        ] {
            assert!(build_profile_path(name).is_err(), "{name}");
        }
        assert!(build_profile_path(&"x".repeat(65)).is_err());
        assert!(build_profile_bytes(&json!([])).is_err());
    }
}
