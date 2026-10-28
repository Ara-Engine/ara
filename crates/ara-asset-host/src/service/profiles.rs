//! Profile-only preparation never opens the importer graph or writes authored files.
use super::{
    inventory::{asset, one},
    links::reject_linked_path,
};
use ara_asset::Cancellation;
use ara_cmd::{
    Batch, Command,
    builtin::{
        BUILD_PROFILE_METADATA, BuildProfileCreate, BuildProfileCreateParams, build_profile_bytes,
        build_profile_path,
    },
};
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::{FileKind, FileState, PreparedInventory};
use ara_host::file_state::FileHistory;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

struct Profile {
    name: String,
    path: String,
    preset: Value,
    bytes: Vec<u8>,
    state: FileState,
}

pub(super) fn prepare(
    root: &Path,
    batch: &Batch,
    cancel: &Cancellation,
) -> Result<PreparedInventory, Vec<Diagnostic>> {
    cancel.check().map_err(asset)?;
    if batch.calls.is_empty() {
        return Err(one(invalid(
            "a build profile batch needs at least one profile",
        )));
    }
    let mut profiles = Vec::new();
    let mut names = BTreeSet::new();
    for (index, call) in batch.calls.iter().enumerate() {
        if call.command != BuildProfileCreate::NAME {
            return Err(one(invalid(
                "build profile batches accept only build_profile.create",
            )
            .command(index)));
        }
        let params: BuildProfileCreateParams = serde_json::from_value(call.params.clone())
            .map_err(|error| one(invalid(error.to_string()).command(index)))?;
        let path = build_profile_path(&params.name).map_err(|error| one(error.command(index)))?;
        let bytes =
            build_profile_bytes(&params.preset).map_err(|error| one(error.command(index)))?;
        if !names.insert(path.clone()) {
            return Err(one(invalid(
                "this batch creates the same normalized build profile more than once",
            )
            .file(&path)
            .command(index)));
        }
        profiles.push(Profile {
            name: params.name.to_ascii_lowercase(),
            path,
            preset: params.preset,
            state: FileState::bytes(&bytes),
            bytes,
        });
    }
    let root = root.canonicalize().map_err(|error| one(io(error)))?;
    let history = FileHistory::new(&root);
    let mut prepared = PreparedInventory::default();
    prepared
        .directories
        .insert(String::new(), history.directory_entries("").map_err(one)?);
    for parent in ["settings", "settings/builds"] {
        observe(&root, &history, &mut prepared, parent)?;
        if let Some(state) = prepared.entries[parent].as_ref() {
            if state.kind != FileKind::Directory {
                return Err(one(
                    invalid("build profile parent must be a directory").file(parent)
                ));
            }
            prepared.directories.insert(
                parent.to_owned(),
                history.directory_entries(parent).map_err(one)?,
            );
        }
    }
    for profile in &profiles {
        cancel.check().map_err(asset)?;
        observe(&root, &history, &mut prepared, &profile.path)?;
        if prepared.entries[&profile.path].is_some() {
            return Err(one(invalid(
                "the build profile already exists; choose another name",
            )
            .file(&profile.path)));
        }
    }
    history.revalidate(&prepared).map_err(one)?;
    let mut metadata = Vec::new();
    for profile in profiles {
        cancel.check().map_err(asset)?;
        if !batch.dry_run {
            for path in [
                ".ara".to_owned(),
                ".ara/file-history".to_owned(),
                format!(".ara/file-history/{}", profile.state.hash),
            ] {
                check_links(&root, &path)?;
            }
            let state = history.put(&profile.bytes).map_err(one)?;
            if state != profile.state || history.read(&state).map_err(one)? != profile.bytes {
                return Err(one(Diagnostic::error(
                    codes::STALE_BASE,
                    "prepared build profile snapshot does not match canonical bytes",
                )
                .file(&profile.path)));
            }
        }
        metadata.push(json!({"name":profile.name,"path":profile.path,"preset":profile.preset,"state":profile.state}));
    }
    prepared.metadata = json!({BUILD_PROFILE_METADATA:metadata});
    cancel.check().map_err(asset)?;
    for path in prepared.entries.keys() {
        check_links(&root, path)?;
    }
    history.revalidate(&prepared).map_err(one)?;
    cancel.check().map_err(asset)?;
    Ok(prepared)
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message)
}
fn io(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(codes::IO, error.to_string())
}

fn check_links(root: &Path, path: &str) -> Result<(), Vec<Diagnostic>> {
    match fs::symlink_metadata(root.join(path)) {
        Ok(_) => reject_linked_path(root, path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(one(io(error).file(path))),
    }
}

fn observe(
    root: &Path,
    history: &FileHistory,
    prepared: &mut PreparedInventory,
    path: &str,
) -> Result<(), Vec<Diagnostic>> {
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    if let Some(children) = prepared.directories.get(parent)
        && let Some(alias) = children
            .iter()
            .find(|child| ara_doc::file_state::same_path(child, path) && child.as_str() != path)
    {
        return Err(one(invalid(format!("`{alias}` conflicts with the lowercase portable path `{path}`; rename it before creating the profile")).file(path)));
    }
    check_links(root, path)?;
    prepared
        .entries
        .insert(path.to_owned(), history.observe(path).map_err(one)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch() -> tempfile::TempDir {
        let root = std::env::var_os("ARA_IMPORT_TEST_ROOT")
            .or_else(|| std::env::var_os("ARA_TEST_ROOT"))
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    PathBuf::from("D:/AraBuild/ux-hub-evidence")
                } else {
                    std::env::temp_dir()
                }
            })
            .join("build-profile-tests");
        fs::create_dir_all(&root).expect("scratch root");
        tempfile::Builder::new()
            .prefix("profile-")
            .tempdir_in(root)
            .expect("isolated scratch")
    }
    fn batch() -> Batch {
        Batch::new(
            "pj_project",
            [(
                BuildProfileCreate::NAME,
                json!({"name":"Desktop","preset":{"platform":"windows","assembly":"Game"}}),
            )],
        )
    }

    #[test]
    fn profile_worker_dry_run_is_read_only_and_full_preparation_stores_exact_bytes() {
        let scratch = scratch();
        let root = scratch.path();
        let mut batch = batch();
        batch.dry_run = true;
        let preview = prepare(root, &batch, &Cancellation::default()).expect("dry run");
        assert!(!root.join(".ara").exists());
        assert!(!root.join("settings").exists());
        assert_eq!(preview.entries.len(), 3);
        assert!(preview.entries.values().all(Option::is_none));
        batch.dry_run = false;
        let ready = prepare(root, &batch, &Cancellation::default()).expect("prepare");
        assert_eq!(preview, ready);
        let row = &ready.metadata[BUILD_PROFILE_METADATA][0];
        assert_eq!(row["name"], "desktop");
        assert_eq!(row["path"], "settings/builds/desktop.export.json");
        let state: FileState = serde_json::from_value(row["state"].clone()).expect("state");
        assert_eq!(
            FileHistory::new(root)
                .read(&state)
                .expect("immutable bytes"),
            build_profile_bytes(&row["preset"]).expect("canonical bytes")
        );
        assert!(
            !root.join("settings").exists(),
            "preparation must never materialize authored files or parents"
        );
        FileHistory::new(root)
            .revalidate(&ready)
            .expect("valid inventory");
    }

    #[test]
    fn profiles_capture_directory_membership_and_refuse_existing_or_duplicate_targets() {
        let scratch = scratch();
        let root = scratch.path();
        fs::create_dir_all(root.join("settings/builds")).expect("parents");
        let mut batch = batch();
        batch.dry_run = true;
        batch.calls.push(ara_cmd::Call {
            command: BuildProfileCreate::NAME.into(),
            params: json!({"name":"Server","preset":{}}),
        });
        let ready = prepare(root, &batch, &Cancellation::default()).expect("multiple profiles");
        assert_eq!(
            ready.metadata[BUILD_PROFILE_METADATA]
                .as_array()
                .expect("rows")
                .len(),
            2
        );
        assert_eq!(ready.directories.len(), 3);
        fs::write(root.join("settings/unrelated.json"), "{}").expect("external directory change");
        assert!(FileHistory::new(root).revalidate(&ready).is_err());
        fs::write(root.join("settings/builds/desktop.export.json"), "existing")
            .expect("existing profile");
        assert!(prepare(root, &batch, &Cancellation::default()).is_err());
        fs::remove_file(root.join("settings/builds/desktop.export.json"))
            .expect("remove test fixture");
        batch.calls[1].params["name"] = "DESKTOP".into();
        assert!(prepare(root, &batch, &Cancellation::default()).is_err());
        assert!(!root.join(".ara").exists());
    }

    #[test]
    fn profile_worker_rejects_file_parents_mixed_batches_and_cancellation_without_writes() {
        let scratch = scratch();
        let root = scratch.path();
        fs::write(root.join("settings"), "file parent").expect("invalid parent fixture");
        assert!(prepare(root, &batch(), &Cancellation::default()).is_err());
        assert!(!root.join(".ara").exists());
        let mut mixed = batch();
        mixed.calls.push(ara_cmd::Call {
            command: "entity.create".into(),
            params: json!({}),
        });
        assert!(prepare(root, &mixed, &Cancellation::default()).is_err());
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(prepare(root, &batch(), &cancel).is_err());
        assert!(!root.join(".ara").exists());
    }

    #[cfg(unix)]
    #[test]
    fn profile_worker_refuses_linked_settings_without_touching_the_external_directory() {
        let outside = scratch();
        let project = scratch();
        std::os::unix::fs::symlink(outside.path(), project.path().join("settings"))
            .expect("link fixture");
        assert!(prepare(project.path(), &batch(), &Cancellation::default()).is_err());
        assert!(
            fs::read_dir(outside.path())
                .expect("outside directory")
                .next()
                .is_none()
        );
        assert!(!project.path().join(".ara").exists());
    }
}
