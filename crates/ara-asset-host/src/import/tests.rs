use super::*;
use ara_asset::{Artifact, ArtifactKind, CookPreset, TargetPlatform};
use std::fs;

struct ImportersFixture {
    model: bool,
}
impl Importers for ImportersFixture {
    fn source(&self, path: &str, _sidecar: Option<&[u8]>) -> Result<Option<SourceAsset>> {
        Ok((self.model && path.ends_with(".gltf")).then(|| binding(path)))
    }
    fn native_document(&self, path: &str) -> bool {
        path.ends_with(".scene") || path.ends_with(".prefab")
    }
}
struct WorkerFixture {
    inputs: Vec<String>,
}
impl SourceWorker for WorkerFixture {
    fn discover(
        &self,
        _binding: &SourceAsset,
        _source: Vec<u8>,
        cancel: &Cancellation,
    ) -> Result<Vec<String>> {
        cancel.check()?;
        Ok(self.inputs.clone())
    }
    fn cook(
        &self,
        _input: ImportInput,
        cancel: &Cancellation,
        _progress: &mut dyn FnMut(ara_asset::Progress),
    ) -> Result<ImportOutput> {
        cancel.check()?;
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "fixture".into(),
                kind: ArtifactKind::Custom,
                bytes: b"cooked".to_vec(),
            }],
        })
    }
}
fn binding(path: &str) -> SourceAsset {
    SourceAsset {
        preset: Some(CookPreset::default()),
        thread_budget: 1,
        id: "as_existing".into(),
        path: path.into(),
        importer: "ara.fixture".into(),
        version: "1".into(),
        settings: serde_json::json!({"keep":"destination settings"}),
        target: TargetPlatform::Linux,
        asset_dependencies: BTreeMap::new(),
    }
}
fn scratch() -> tempfile::TempDir {
    let root = std::env::var_os("ARA_IMPORT_TEST_ROOT")
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .join("asset-host-import-tests");
    fs::create_dir_all(&root).expect("target test root");
    let root = tempfile::tempdir_in(root).expect("isolated import");
    for path in ["external", "game", "derived"] {
        fs::create_dir(root.path().join(path)).expect("test folder");
    }
    root
}
fn inventory() -> DestinationInventory {
    DestinationInventory {
        token: "complete-v1".into(),
        case_insensitive: false,
        entries: BTreeMap::new(),
    }
}
fn context<'a>(
    root: &'a Path,
    stage: &'a Path,
    inventory: &'a DestinationInventory,
    worker: &'a dyn SourceWorker,
    importers: &'a dyn Importers,
    space: &'a (dyn Fn(&Path, u64) -> Result<()> + Send + Sync),
) -> Context<'a> {
    Context {
        project_root: root.parent().expect("fixture project root"),
        authored_root: root,
        staging: stage,
        inventory,
        worker,
        importers,
        limits: Limits {
            bytes: 16 * 1024 * 1024,
            entries: 256,
        },
        check_space: space,
    }
}

#[test]
fn staging_and_discard_never_change_authored_bytes_and_keep_empty_folders() {
    let root = scratch();
    let source = root.path().join("external/pack");
    fs::create_dir_all(source.join("empty")).expect("empty authored folder");
    fs::write(source.join("image.bin"), b"incoming").expect("source");
    let authored = root.path().join("game");
    fs::write(authored.join("existing.bin"), b"existing").expect("destination sentinel");
    let stage = root.path().join("derived/job");
    let worker = WorkerFixture { inputs: vec![] };
    let importers = ImportersFixture { model: false };
    let prepared = prepare(
        Request {
            dry_run: false,
            sources: vec![source],
            destination: "art".into(),
            conflict: ImportConflict::Rename,
        },
        context(
            &authored,
            &stage,
            &inventory(),
            &worker,
            &importers,
            &|_, _| Ok(()),
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("prepare");
    assert!(
        prepared.entries.iter().any(
            |entry| entry.destination == "art/pack/empty" && entry.kind == EntryKind::Directory
        )
    );
    let file = prepared
        .entries
        .iter()
        .find(|entry| entry.destination == "art/pack/image.bin")
        .expect("staged file");
    assert_eq!(file.digest, Some(Digest::of(b"incoming")));
    assert_eq!(
        fs::read(authored.join("existing.bin")).expect("unchanged"),
        b"existing"
    );
    assert!(!authored.join("art").exists());
    prepared
        .verify(&Cancellation::default())
        .expect("immutable staged bytes");
    prepared.discard().expect("cancel/discard");
    assert!(!stage.exists());
}

#[test]
fn cancelling_byte_copy_discards_staging_and_preserves_destination() {
    let root = scratch();
    let source = root.path().join("external/large.bin");
    fs::write(&source, vec![7; 192 * 1024]).expect("source");
    let authored = root.path().join("game");
    let stage = root.path().join("derived/cancel");
    let cancel = Cancellation::default();
    let worker = WorkerFixture { inputs: vec![] };
    let importers = ImportersFixture { model: false };
    let result = prepare(
        Request {
            dry_run: false,
            sources: vec![source.clone()],
            destination: String::new(),
            conflict: ImportConflict::Rename,
        },
        context(
            &authored,
            &stage,
            &inventory(),
            &worker,
            &importers,
            &|_, _| Ok(()),
        ),
        &cancel,
        &mut |progress| {
            if progress.phase == Phase::Staging && progress.received > 0 {
                cancel.cancel();
            }
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(!stage.exists());
    assert!(!authored.join("large.bin").exists());
    assert_eq!(
        fs::metadata(source).expect("source untouched").len(),
        192 * 1024
    );
}

#[test]
fn byte_budget_and_disk_admission_fail_before_creating_staging() {
    let root = scratch();
    let source = root.path().join("external/file.bin");
    fs::write(&source, b"bytes").expect("source");
    let authored = root.path().join("game");
    let stage = root.path().join("derived/budget");
    let inventory = inventory();
    let worker = WorkerFixture { inputs: vec![] };
    let importers = ImportersFixture { model: false };
    let mut limited = context(
        &authored,
        &stage,
        &inventory,
        &worker,
        &importers,
        &|_, _| Ok(()),
    );
    limited.limits.bytes = 1;
    assert!(
        prepare(
            Request {
                dry_run: false,
                sources: vec![source.clone()],
                destination: String::new(),
                conflict: ImportConflict::Rename
            },
            limited,
            &Cancellation::default(),
            &mut |_| {}
        )
        .is_err()
    );
    assert!(!stage.exists());
    assert!(
        prepare(
            Request {
                dry_run: false,
                sources: vec![source],
                destination: String::new(),
                conflict: ImportConflict::Rename
            },
            context(
                &authored,
                &stage,
                &inventory,
                &worker,
                &importers,
                &|_, _| Err(Error::Invalid("not enough disk space".into()))
            ),
            &Cancellation::default(),
            &mut |_| {}
        )
        .is_err()
    );
    assert!(!stage.exists());
}

#[test]
fn rename_is_compound_extension_safe_and_replace_merges_without_deleting_siblings() {
    assert_eq!(
        planner::suffixed("ui/menu.types.json", 2, EntryKind::File),
        "ui/menu (2).types.json"
    );
    assert_eq!(
        planner::suffixed("box.png.import.json", 3, EntryKind::File),
        "box.png (3).import.json"
    );
    assert_eq!(
        planner::suffixed("v1.assets", 2, EntryKind::Directory),
        "v1.assets (2)"
    );
    let root = scratch();
    let source = root.path().join("external/pack");
    fs::create_dir(&source).expect("source folder");
    fs::write(source.join("new.bin"), b"new").expect("source");
    let mut inventory = inventory();
    inventory.entries.insert(
        "pack".into(),
        ExistingEntry {
            kind: EntryKind::Directory,
            bytes: 0,
            digest: None,
            asset_id: None,
            binding: None,
        },
    );
    inventory.entries.insert(
        "pack/keep.bin".into(),
        ExistingEntry {
            kind: EntryKind::File,
            bytes: 4,
            digest: Some(Digest::of(b"keep")),
            asset_id: None,
            binding: None,
        },
    );
    let scan = external::scan(&[source], &Cancellation::default(), 64, |_| {}).expect("scan");
    let merged = planner::plan(&scan.entries, "", ImportConflict::Replace, &inventory)
        .expect("explicit merge");
    assert!(
        merged
            .iter()
            .any(|entry| entry.destination == "pack" && entry.disposition == Disposition::Merged)
    );
    assert!(
        !merged
            .iter()
            .any(|entry| entry.destination == "pack/keep.bin")
    );
    let skipped =
        planner::plan(&scan.entries, "", ImportConflict::Skip, &inventory).expect("skip subtree");
    assert!(
        skipped
            .iter()
            .all(|entry| entry.disposition == Disposition::Skipped)
    );
}

#[test]
fn external_companions_require_explicit_authorization_and_worker_validation() {
    let root = scratch();
    let model = root.path().join("external/model.gltf");
    let buffer = root.path().join("external/model.bin");
    fs::write(&model, b"model").expect("model");
    fs::write(&buffer, b"mesh").expect("buffer");
    let authored = root.path().join("game");
    let worker = WorkerFixture {
        inputs: vec!["model.bin".into()],
    };
    let importers = ImportersFixture { model: true };
    let stage = root.path().join("derived/rejected");
    let error = prepare(
        Request {
            dry_run: false,
            sources: vec![model.clone()],
            destination: "models".into(),
            conflict: ImportConflict::Rename,
        },
        context(
            &authored,
            &stage,
            &inventory(),
            &worker,
            &importers,
            &|_, _| Ok(()),
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .err()
    .expect("unselected companion");
    assert!(error.to_string().contains("explicitly selected"));
    assert!(!stage.exists());
    let stage = root.path().join("derived/authorized");
    let prepared = prepare(
        Request {
            dry_run: false,
            sources: vec![model, buffer],
            destination: "models".into(),
            conflict: ImportConflict::Rename,
        },
        context(
            &authored,
            &stage,
            &inventory(),
            &worker,
            &importers,
            &|_, _| Ok(()),
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("explicit source and companion");
    assert_eq!(prepared.sources.len(), 1);
    assert_eq!(prepared.sources[0].input.dependencies["model.bin"], b"mesh");
    assert!(!authored.join("models").exists());
    prepared.discard().expect("discard");
}

#[test]
fn rename_conflict_regroups_binary_model_and_companion_without_overwrite() {
    let root = scratch();
    let model = root.path().join("external/model.gltf");
    let texture = root.path().join("external/albedo.png");
    fs::write(&model, b"model").expect("model");
    fs::write(&texture, b"new pixels").expect("texture");
    let authored = root.path().join("game");
    fs::write(authored.join("albedo.png"), b"old pixels").expect("existing");
    let mut inventory = inventory();
    inventory.entries.insert(
        "albedo.png".into(),
        ExistingEntry {
            kind: EntryKind::File,
            bytes: 10,
            digest: Some(Digest::of(b"old pixels")),
            asset_id: Some("as_old".into()),
            binding: None,
        },
    );
    let stage = root.path().join("derived/regroup");
    let worker = WorkerFixture {
        inputs: vec!["albedo.png".into()],
    };
    let importers = ImportersFixture { model: true };
    let prepared = prepare(
        Request {
            dry_run: false,
            sources: vec![model, texture],
            destination: String::new(),
            conflict: ImportConflict::Rename,
        },
        context(
            &authored,
            &stage,
            &inventory,
            &worker,
            &importers,
            &|_, _| Ok(()),
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("rename group");
    assert!(
        prepared
            .entries
            .iter()
            .any(|entry| entry.destination == "Imported/model.gltf")
    );
    assert!(
        prepared
            .entries
            .iter()
            .any(|entry| entry.destination == "Imported/albedo.png")
    );
    assert_eq!(
        prepared.sources[0].input.dependencies["albedo.png"],
        b"new pixels"
    );
    assert_eq!(
        fs::read(authored.join("albedo.png")).expect("existing preserved"),
        b"old pixels"
    );
    prepared.discard().expect("discard");
}

#[test]
fn replace_preserves_destination_binding_and_foreign_sidecar_is_not_a_raw_copy() {
    let root = scratch();
    let model = root.path().join("external/model.gltf");
    fs::write(&model, b"new model").expect("model");
    fs::write(
        root.path().join("external/model.gltf.import.json"),
        b"foreign ID/settings",
    )
    .expect("foreign sidecar");
    let authored = root.path().join("game");
    let stage = root.path().join("derived/replace");
    let mut inventory = inventory();
    inventory.entries.insert(
        "model.gltf".into(),
        ExistingEntry {
            kind: EntryKind::File,
            bytes: 9,
            digest: Some(Digest::of(b"old model")),
            asset_id: Some("as_existing".into()),
            binding: Some(binding("model.gltf")),
        },
    );
    inventory.entries.insert(
        "model.gltf.import.json".into(),
        ExistingEntry {
            kind: EntryKind::File,
            bytes: 12,
            digest: Some(Digest::of(b"own settings")),
            asset_id: Some("as_existing".into()),
            binding: None,
        },
    );
    let worker = WorkerFixture { inputs: vec![] };
    let importers = ImportersFixture { model: true };
    let prepared = prepare(
        Request {
            dry_run: false,
            sources: vec![model, root.path().join("external/model.gltf.import.json")],
            destination: String::new(),
            conflict: ImportConflict::Replace,
        },
        context(
            &authored,
            &stage,
            &inventory,
            &worker,
            &importers,
            &|_, _| Ok(()),
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("replace");
    assert_eq!(prepared.sources[0].binding.id, "as_existing");
    assert_eq!(
        prepared.sources[0].input.settings,
        serde_json::json!({"keep":"destination settings"})
    );
    let sidecar = prepared
        .entries
        .iter()
        .find(|entry| entry.destination.ends_with(".import.json"))
        .expect("sidecar");
    assert_eq!(sidecar.disposition, Disposition::Skipped);
    assert!(sidecar.native_document);
    assert_eq!(
        prepared.sources[0].foreign_sidecar.as_deref(),
        Some(b"foreign ID/settings".as_slice())
    );
    prepared.discard().expect("discard");
}

#[test]
fn whole_projects_are_refused_and_hidden_system_entries_do_not_hide_authored_folder_names() {
    let root = scratch();
    let source = root.path().join("external/pack");
    fs::create_dir_all(source.join(".git")).expect("derived dir");
    fs::create_dir_all(source.join(".ara")).expect("derived cache");
    fs::create_dir_all(source.join("builds")).expect("outputs");
    fs::create_dir_all(source.join("empty")).expect("empty");
    fs::write(source.join(".git/config"), b"ignore").expect("derived");
    fs::write(source.join(".ara/cache"), b"ignore").expect("cache");
    fs::write(source.join("builds/player.exe"), b"explicit authored data")
        .expect("authored folder named builds");
    for name in ["target", "builds", "packages", "node_modules"] {
        fs::create_dir_all(source.join("feature").join(name)).expect("authored nested directory");
        fs::write(
            source.join("feature").join(name).join("asset.bin"),
            b"authored",
        )
        .expect("authored data");
    }
    let scan = external::scan(
        std::slice::from_ref(&source),
        &Cancellation::default(),
        64,
        |_| {},
    )
    .expect("safe content");
    assert!(
        !scan
            .entries
            .iter()
            .any(|entry| entry.relative.contains(".git") || entry.relative.contains(".ara"))
    );
    assert!(
        scan.entries
            .iter()
            .any(|entry| entry.relative == "pack/builds/player.exe")
    );
    for name in ["target", "builds", "packages", "node_modules"] {
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.relative == format!("pack/feature/{name}/asset.bin"))
        );
    }
    assert!(
        scan.entries
            .iter()
            .any(|entry| entry.relative.ends_with("empty"))
    );
    fs::write(source.join("ara.project.json"), b"{}").expect("project marker");
    assert!(external::scan(&[source], &Cancellation::default(), 64, |_| {}).is_err());
}

#[test]
fn unicode_import_conflicts_use_shared_alias_policy_before_deterministic_rename() {
    let incoming = external::Incoming {
        group: 0,
        source: PathBuf::from("external/source.png"),
        relative: "ς.png".into(),
        kind: EntryKind::File,
        bytes: 8,
        modified: None,
    };
    let mut inventory = inventory();
    inventory.case_insensitive = true;
    inventory.entries.insert(
        "Σ.png".into(),
        ExistingEntry {
            kind: EntryKind::File,
            bytes: 8,
            digest: Some(Digest::of(b"existing")),
            asset_id: None,
            binding: None,
        },
    );
    let (planned, conflicts) = planner::plan_with_conflicts(
        std::slice::from_ref(&incoming),
        "",
        ImportConflict::Rename,
        &inventory,
    )
    .expect("case-aware rename");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(planned[0].destination, "ς (2).png");
    assert_eq!(planned[0].disposition, Disposition::Renamed);
    inventory.case_insensitive = false;
    let (planned, conflicts) =
        planner::plan_with_conflicts(&[incoming], "", ImportConflict::Rename, &inventory)
            .expect("distinct sensitive names");
    assert!(conflicts.is_empty());
    assert_eq!(planned[0].destination, "ς.png");
}

#[test]
#[cfg(unix)]
fn symlink_files_and_folders_are_rejected_without_following_them() {
    use std::os::unix::fs::symlink;
    let root = scratch();
    fs::write(root.path().join("private.bin"), b"outside").expect("outside");
    let link = root.path().join("external/link.bin");
    symlink(root.path().join("private.bin"), &link).expect("test link");
    assert!(external::scan(&[link], &Cancellation::default(), 64, |_| {}).is_err());
    assert_eq!(
        fs::read(root.path().join("private.bin")).expect("outside preserved"),
        b"outside"
    );
}

mod preflight;
