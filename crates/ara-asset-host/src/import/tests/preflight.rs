use super::*;

fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(root: &Path, path: &Path, output: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).expect("read test tree") {
            let path = entry.expect("entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("inside test root")
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                output.insert(relative, None);
                walk(root, &path, output);
            } else {
                output.insert(relative, Some(fs::read(path).expect("test bytes")));
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
fn request(source: &Path, conflict: ImportConflict, dry_run: bool) -> Request {
    Request {
        sources: vec![source.to_owned()],
        destination: String::new(),
        conflict,
        dry_run,
    }
}

struct NoCook;
impl SourceWorker for NoCook {
    fn discover(
        &self,
        _binding: &SourceAsset,
        _bytes: Vec<u8>,
        cancel: &Cancellation,
    ) -> Result<Vec<String>> {
        cancel.check()?;
        Ok(vec!["model.bin".into()])
    }
    fn cook(
        &self,
        _input: ImportInput,
        _cancel: &Cancellation,
        _progress: &mut dyn FnMut(ara_asset::Progress),
    ) -> Result<ImportOutput> {
        panic!("dry_run must never cook")
    }
}

#[test]
fn readonly_preflight_validates_worker_companions_and_preserves_both_trees_byte_for_byte() {
    let root = scratch();
    let folder = root.path().join("external/pack");
    fs::create_dir(&folder).expect("pack");
    fs::create_dir(folder.join("empty")).expect("empty");
    fs::write(folder.join("model.gltf"), b"model").expect("model");
    fs::write(folder.join("model.bin"), b"mesh").expect("companion");
    let authored = root.path().join("game");
    fs::write(authored.join("keep.bin"), b"keep").expect("destination");
    let stage = root.path().join("derived/preview");
    let inventory = inventory();
    let importer = ImportersFixture { model: true };
    let before = tree(root.path());
    let plan = preflight(
        &request(&folder, ImportConflict::Rename, true),
        &context(
            &authored,
            &stage,
            &inventory,
            &NoCook,
            &importer,
            &|_, _| Ok(()),
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("readonly validated plan");
    assert!(plan.conflicts.is_empty());
    assert!(
        plan.planned
            .iter()
            .any(|entry| entry.destination == "pack/empty")
    );
    assert!(plan.planned.iter().all(|entry| entry.staged.is_none()));
    assert!(!stage.exists());
    assert_eq!(tree(root.path()), before);
}

#[test]
fn conflicts_are_actual_collision_rows_even_after_rename_replace_or_skip() {
    let root = scratch();
    let source = root.path().join("external/icon.bin");
    fs::write(&source, b"incoming").expect("source");
    let authored = root.path().join("game");
    fs::write(authored.join("icon.bin"), b"existing").expect("destination");
    let mut inventory = inventory();
    inventory.entries.insert(
        "icon.bin".into(),
        ExistingEntry {
            kind: EntryKind::File,
            bytes: 8,
            digest: Some(Digest::of(b"existing")),
            asset_id: Some("as_existing".into()),
            binding: None,
        },
    );
    let worker = WorkerFixture { inputs: vec![] };
    let importer = ImportersFixture { model: false };
    let before = tree(root.path());
    for policy in [
        ImportConflict::Rename,
        ImportConflict::Replace,
        ImportConflict::Skip,
    ] {
        let stage = root.path().join("derived/preview");
        let plan = preflight(
            &request(&source, policy, true),
            &context(
                &authored,
                &stage,
                &inventory,
                &worker,
                &importer,
                &|_, _| Ok(()),
            ),
            &Cancellation::default(),
            &mut |_| {},
        )
        .expect("preview");
        assert_eq!(plan.conflicts.len(), 1);
        let conflict = &plan.conflicts[0];
        assert_eq!(conflict.source, source.canonicalize().expect("source"));
        assert_eq!(conflict.destination_logical_path, "icon.bin");
        assert_eq!(conflict.existing_asset_id.as_deref(), Some("as_existing"));
        assert_eq!(conflict.kind.as_str(), "file");
        let planned = &plan.planned[0];
        match policy {
            ImportConflict::Rename => assert_eq!(planned.destination, "icon (2).bin"),
            ImportConflict::Replace => assert_eq!(planned.disposition, Disposition::Replaced),
            ImportConflict::Skip => assert_eq!(planned.disposition, Disposition::Skipped),
        }
        assert_eq!(tree(root.path()), before);
    }
}

#[test]
fn conflict_kind_is_precise_for_unowned_directories_type_mismatches_and_sidecars() {
    let root = scratch();
    let source = root.path().join("external/file.bin");
    fs::write(&source, b"source").expect("source");
    let authored = root.path().join("game");
    let worker = WorkerFixture { inputs: vec![] };
    let importer = ImportersFixture { model: false };
    for (path, entry, kind) in [
        (
            "file.bin",
            ExistingEntry {
                kind: EntryKind::Directory,
                bytes: 0,
                digest: None,
                asset_id: None,
                binding: None,
            },
            "type_mismatch",
        ),
        (
            "file.bin.import.json",
            ExistingEntry {
                kind: EntryKind::File,
                bytes: 4,
                digest: Some(Digest::of(b"own")),
                asset_id: Some("as_own".into()),
                binding: None,
            },
            "sidecar",
        ),
    ] {
        let mut inventory = inventory();
        inventory.entries.insert(path.into(), entry);
        let stage = root.path().join("derived/kind");
        let plan = preflight(
            &request(&source, ImportConflict::Rename, true),
            &context(
                &authored,
                &stage,
                &inventory,
                &worker,
                &importer,
                &|_, _| Ok(()),
            ),
            &Cancellation::default(),
            &mut |_| {},
        )
        .expect("kind preview");
        assert_eq!(plan.conflicts[0].kind.as_str(), kind);
    }
    let folder = root.path().join("external/folder");
    fs::create_dir(&folder).expect("folder");
    let mut inventory = inventory();
    inventory.entries.insert(
        "folder".into(),
        ExistingEntry {
            kind: EntryKind::Directory,
            bytes: 0,
            digest: None,
            asset_id: None,
            binding: None,
        },
    );
    let stage = root.path().join("derived/folder");
    let plan = preflight(
        &request(&folder, ImportConflict::Replace, true),
        &context(
            &authored,
            &stage,
            &inventory,
            &worker,
            &importer,
            &|_, _| Ok(()),
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("directory preview");
    assert_eq!(plan.conflicts[0].kind.as_str(), "directory");
    assert!(plan.conflicts[0].existing_asset_id.is_none());
}

#[test]
fn invalid_params_cancellation_and_limits_have_the_same_diagnostics_in_both_modes() {
    let root = scratch();
    let source = root.path().join("external/file.bin");
    fs::write(&source, b"source").expect("source");
    let authored = root.path().join("game");
    let worker = WorkerFixture { inputs: vec![] };
    let importer = ImportersFixture { model: false };
    let inventory = inventory();
    let before = tree(root.path());
    for scenario in [
        "path", "reserved", "quote", "control", "long", "cancel", "bytes", "entries",
    ] {
        let mut errors = Vec::new();
        for dry_run in [true, false] {
            let stage = root.path().join("derived/check");
            let mut context = context(
                &authored,
                &stage,
                &inventory,
                &worker,
                &importer,
                &|_, _| Ok(()),
            );
            let mut request = request(&source, ImportConflict::Rename, dry_run);
            let cancel = Cancellation::default();
            match scenario {
                "path" => request.destination = "../outside".into(),
                "reserved" => request.destination = "props/COM¹".into(),
                "quote" => request.destination = "props/bad\"name".into(),
                "control" => request.destination = "props/line\nbreak".into(),
                "long" => request.destination = "a".repeat(256),
                "cancel" => cancel.cancel(),
                "bytes" => context.limits.bytes = 1,
                "entries" => context.limits.entries = 0,
                _ => {}
            }
            let error = run(request, context, &cancel, &mut |_| {})
                .err()
                .expect("validation failure");
            errors.push(error.to_string());
            assert!(!stage.exists());
        }
        assert_eq!(
            errors[0], errors[1],
            "same readonly/execution admission: {scenario}"
        );
        assert_eq!(tree(root.path()), before);
    }
}

#[test]
fn protected_legacy_destinations_fail_readonly_admission_while_modern_authored_names_remain_free() {
    let root = scratch();
    let source = root.path().join("external/file.bin");
    fs::write(&source, b"source").expect("source");
    let authored = root.path().join("game");
    let mut inventory = inventory();
    inventory.case_insensitive = true;
    let worker = WorkerFixture { inputs: vec![] };
    let importer = ImportersFixture { model: false };
    let before = tree(root.path());
    let mut errors = Vec::new();
    for dry_run in [true, false] {
        let stage = root.path().join("derived/protected");
        let mut context = context(
            &authored,
            &stage,
            &inventory,
            &worker,
            &importer,
            &|_, _| Ok(()),
        );
        context.project_root = &authored;
        let mut request = request(&source, ImportConflict::Rename, dry_run);
        request.destination = "Settings".into();
        errors.push(
            run(request, context, &Cancellation::default(), &mut |_| {})
                .err()
                .expect("protected root")
                .to_string(),
        );
        assert!(!stage.exists());
    }
    assert_eq!(errors[0], errors[1]);
    assert!(errors[0].contains("protected legacy root"));
    assert_eq!(tree(root.path()), before);
    let stage = root.path().join("derived/modern");
    let context = context(
        &authored,
        &stage,
        &inventory,
        &worker,
        &importer,
        &|_, _| Ok(()),
    );
    let mut request = request(&source, ImportConflict::Rename, true);
    request.destination = "Settings/packages/builds".into();
    preflight(&request, &context, &Cancellation::default(), &mut |_| {})
        .expect("nested authored names allowed");
    assert_eq!(tree(root.path()), before);
}

#[test]
fn missing_or_unauthorized_companions_are_identical_errors_without_staging_in_dryrun() {
    let root = scratch();
    let source = root.path().join("external/model.gltf");
    fs::write(&source, b"model").expect("model");
    fs::write(root.path().join("external/model.bin"), b"unselected").expect("unselected input");
    let authored = root.path().join("game");
    let inventory = inventory();
    let worker = WorkerFixture {
        inputs: vec!["model.bin".into()],
    };
    let importer = ImportersFixture { model: true };
    let before = tree(root.path());
    let mut errors = Vec::new();
    for dry_run in [true, false] {
        let stage = root.path().join("derived/companion");
        let error = run(
            request(&source, ImportConflict::Rename, dry_run),
            context(
                &authored,
                &stage,
                &inventory,
                &worker,
                &importer,
                &|_, _| Ok(()),
            ),
            &Cancellation::default(),
            &mut |_| {},
        )
        .err()
        .expect("explicit source authorization");
        errors.push(error.to_string());
        assert!(!stage.exists());
    }
    assert_eq!(errors[0], errors[1]);
    assert!(errors[0].contains("explicitly selected"));
    assert_eq!(tree(root.path()), before);
}

#[test]
fn legacy_staging_is_owned_only_inside_project_ara_and_preview_still_writes_nothing() {
    let root = scratch();
    let source = root.path().join("external/file.bin");
    fs::write(&source, b"source").expect("source");
    let authored = root.path().join("game");
    fs::create_dir(authored.join(".ara")).expect("legacy derived root");
    let stage = authored.join(".ara/job");
    let inventory = inventory();
    let worker = WorkerFixture { inputs: vec![] };
    let importer = ImportersFixture { model: false };
    let before = tree(root.path());
    let mut preview_context = context(
        &authored,
        &stage,
        &inventory,
        &worker,
        &importer,
        &|_, _| Ok(()),
    );
    preview_context.project_root = &authored;
    preflight(
        &request(&source, ImportConflict::Rename, true),
        &preview_context,
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("legacy readonly admission");
    assert_eq!(tree(root.path()), before);
    let mut actual_context = context(
        &authored,
        &stage,
        &inventory,
        &worker,
        &importer,
        &|_, _| Ok(()),
    );
    actual_context.project_root = &authored;
    let prepared = prepare(
        request(&source, ImportConflict::Rename, false),
        actual_context,
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("legacy derived staging");
    assert!(stage.is_dir());
    assert!(!authored.join("file.bin").exists());
    prepared.discard().expect("discard legacy stage");
    assert_eq!(tree(root.path()), before);
}

#[test]
fn foreign_sidecar_validation_and_mid_read_cancel_do_not_gain_mutation_paths_in_preview() {
    struct RejectSidecar;
    impl Importers for RejectSidecar {
        fn source(&self, _path: &str, sidecar: Option<&[u8]>) -> Result<Option<SourceAsset>> {
            assert!(sidecar.is_some());
            Err(Error::Invalid(
                "foreign sidecar settings are invalid".into(),
            ))
        }
        fn native_document(&self, _path: &str) -> bool {
            false
        }
    }
    let root = scratch();
    let source = root.path().join("external/model.dat");
    let sidecar = root.path().join("external/model.dat.import.json");
    fs::write(&source, b"source").expect("source");
    fs::write(&sidecar, b"invalid sidecar").expect("sidecar");
    let authored = root.path().join("game");
    let inventory = inventory();
    let worker = WorkerFixture { inputs: vec![] };
    let before = tree(root.path());
    let mut errors = Vec::new();
    for dry_run in [true, false] {
        let stage = root.path().join("derived/sidecar");
        let request = Request {
            sources: vec![source.clone(), sidecar.clone()],
            destination: String::new(),
            conflict: ImportConflict::Rename,
            dry_run,
        };
        let error = run(
            request,
            context(
                &authored,
                &stage,
                &inventory,
                &worker,
                &RejectSidecar,
                &|_, _| Ok(()),
            ),
            &Cancellation::default(),
            &mut |_| {},
        )
        .err()
        .expect("same settings validation");
        errors.push(error.to_string());
        assert!(!stage.exists());
    }
    assert_eq!(errors[0], errors[1]);
    assert_eq!(tree(root.path()), before);
    let stage = root.path().join("derived/cancel-read");
    let cancel = Cancellation::default();
    let error = preflight(
        &request(&source, ImportConflict::Rename, true),
        &context(
            &authored,
            &stage,
            &inventory,
            &worker,
            &ImportersFixture { model: false },
            &|_, _| Ok(()),
        ),
        &cancel,
        &mut |progress| {
            if progress.received > 0 {
                cancel.cancel();
            }
        },
    )
    .err()
    .expect("cancel readonly bytes");
    assert!(matches!(error, Error::Cancelled));
    assert_eq!(tree(root.path()), before);
}

#[test]
fn replacement_space_estimate_accounts_for_undo_copy_even_for_tiny_incoming_file() {
    let root = scratch();
    let source = root.path().join("external/file.bin");
    fs::write(&source, b"new").expect("source");
    let authored = root.path().join("game");
    let stage = root.path().join("derived/space");
    let mut inventory = inventory();
    inventory.entries.insert(
        "file.bin".into(),
        ExistingEntry {
            kind: EntryKind::File,
            bytes: 8 * 1024 * 1024,
            digest: Some(Digest::of(b"existing")),
            asset_id: None,
            binding: None,
        },
    );
    let worker = WorkerFixture { inputs: vec![] };
    let importer = ImportersFixture { model: false };
    let plan = preflight(
        &request(&source, ImportConflict::Replace, true),
        &context(
            &authored,
            &stage,
            &inventory,
            &worker,
            &importer,
            &|_, required| {
                assert!(required >= 8 * 1024 * 1024);
                Ok(())
            },
        ),
        &Cancellation::default(),
        &mut |_| {},
    )
    .expect("estimated undo reserve");
    assert!(plan.estimated_required_bytes >= 8 * 1024 * 1024);
}
