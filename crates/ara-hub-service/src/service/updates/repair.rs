//! Signed engine repair keeps the old install until the registry commit succeeds.
use super::*;

impl Hub {
    pub(super) fn engine_preflight(&self, version: String) -> Result<Value, RpcError> {
        ara_update::version(&version).map_err(rpc)?;
        let release = self
            .updates
            .releases
            .iter()
            .find(|release| {
                release.manifest().kind == Kind::Engine && release.manifest().version == version
            })
            .ok_or_else(|| invalid("check for releases before planning this installation"))?;
        let asset = release.archive().map_err(rpc)?;
        let destination = self.updates.config.engines.join(&version);
        let available = ara_update::check_space(&destination, 0).map_err(rpc)?;
        let download = ara_update::check_space(&self.updates.staging, 0).map_err(rpc)?;
        // The v1 signed protocol has no expansion size. Present this as an estimate;
        // extraction still checks the exact ZIP total before writing a file.
        Ok(json!({"destination":destination,"downloadBytes":asset.size,
            "requiredBytes":asset.size.saturating_mul(4),"availableBytes":available.available_bytes,
            "downloadAvailableBytes":download.available_bytes,"downloadRequiredBytes":asset.size,
            "spaceBasis":"estimate","channel":release.manifest().channel}))
    }

    pub(super) fn validate_repair(&self, version: &str) -> Result<EngineBuild, RpcError> {
        let engine = self.registry.engine(version)?.clone();
        let source = engine
            .executable
            .parent()
            .ok_or_else(|| invalid("engine has no installation directory"))?;
        let marker: Value =
            serde_json::from_slice(&fs::read(source.join("engine.json")).map_err(io_error)?)
                .map_err(io_error)?;
        if marker["format"] != "ara.engine/1" || marker["version"] != version {
            return Err(invalid(
                "only a packaged engine can be repaired; rebuild custom development engines from their source",
            ));
        }
        let destination = self.updates.config.engines.join(version);
        self.ensure_unused(version, &destination)?;
        if self
            .updates
            .jobs
            .values()
            .any(|job| job.version == version && job.state == "running" && job.kind != "repair")
        {
            return Err(invalid(
                "wait for this version's current operation before repairing",
            ));
        }
        if destination.exists() {
            let proof: Value = serde_json::from_slice(
                &fs::read(destination.join(".ara-managed.json")).map_err(io_error)?,
            )
            .map_err(io_error)?;
            if !same_executable(&engine, &destination)
                || proof["version"] != version
                || fs::symlink_metadata(&destination)
                    .map_err(io_error)?
                    .file_type()
                    .is_symlink()
            {
                return Err(invalid(
                    "repair destination contains another installation; it was preserved",
                ));
            }
        }
        Ok(engine)
    }

    pub(super) fn commit_repair(&mut self, id: String, result: Result<PathBuf, ara_update::Error>) {
        let Some(mut job) = self.updates.jobs.remove(&id) else {
            return;
        };
        let staged = result.as_ref().ok().cloned();
        let result = result.and_then(|staged| {
            job.cancel.check()?;
            let previous = job.repair_source.as_ref().ok_or_else(|| failure("repair lost its original engine identity"))?;
            let current = self.registry.engine(&job.version).map_err(update_error)?;
            if current.executable != previous.executable || current.editor != previous.editor {
                return Err(failure("the engine registration changed during repair; the new registration was preserved"));
            }
            let destination = self.updates.config.engines.join(&job.version);
            self.ensure_unused(&job.version, &destination).map_err(update_error)?;
            let old_marker: Value = serde_json::from_slice(&fs::read(previous.executable.with_file_name("engine.json"))?)?;
            let new_marker: Value = serde_json::from_slice(&fs::read(staged.join("engine.json"))?)?;
            if old_marker.get("sourceCommit").is_some() && old_marker["sourceCommit"] != new_marker["sourceCommit"] {
                return Err(failure("the release differs from this engine's pinned source build; install the new version separately"));
            }
            let backup = destination.with_file_name(format!(".repair-backup-{}", job.version));
            if backup.exists() { return Err(failure("an unfinished repair backup exists; inspect it before retrying")); }
            let had_previous = destination.exists();
            if had_previous { fs::rename(&destination, &backup)?; }
            if let Err(error) = fs::rename(&staged, &destination) {
                if had_previous { fs::rename(&backup, &destination)?; }
                return Err(error.into());
            }
            let old_registry = self.registry.engines.clone();
            let engine = EngineBuild { version: job.version.clone(), executable: destination.join("ara.exe"), editor: Some(destination.join("ara-editor.exe")) };
            self.registry.engines.retain(|engine| engine.version != job.version);
            self.registry.engines.push(engine.clone());
            if let Err(error) = self.registry.save(&self.config.directory) {
                self.registry.engines = old_registry;
                // Only renames run on the authority thread; cleanup stays on a worker.
                fs::rename(&destination, &staged)?;
                if had_previous { fs::rename(&backup, &destination)?; }
                return Err(update_error(error));
            }
            job.result = Some(json!({"engine":engine,"repaired":job.version}));
            job.phase = "finishing".into();
            job.cancellable = false;
            Ok(had_previous.then_some(backup))
        });
        match result {
            Ok(backup) => {
                let tx = self.updates.tx.clone();
                let worker_id = id.clone();
                self.updates.jobs.insert(id.clone(), job);
                let cleanup = std::thread::Builder::new()
                    .name(format!("ara-{id}-cleanup"))
                    .spawn(move || {
                        background_priority();
                        let result = backup
                            .map_or(Ok(()), fs::remove_dir_all)
                            .map_err(Into::into);
                        let _ = tx.send(Message::RepairCleaned(worker_id, result));
                    });
                if let Err(error) = cleanup {
                    self.finish_repair(id, Err(error.into()));
                }
                self.changed("engine", None);
            }
            Err(error) => {
                job.state = if error.code == ara_update::CANCELLED {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                job.phase = job.state.clone();
                job.cancellable = false;
                job.error = Some(error.diagnostic());
                self.updates.jobs.insert(id, job);
                if let Some(staged) = staged {
                    let _ = std::thread::Builder::new()
                        .name("ara-repair-discard".into())
                        .spawn(move || {
                            let _ = fs::remove_dir_all(staged);
                        });
                }
            }
        }
        self.changed("progress", None);
    }

    pub(super) fn finish_repair(&mut self, id: String, result: Result<(), ara_update::Error>) {
        if let Some(job) = self.updates.jobs.get_mut(&id) {
            job.state = "complete".into();
            job.phase = "complete".into();
            job.cancellable = false;
            if let Err(error) = result
                && let Some(value) = job.result.as_mut()
            {
                value["cleanupWarning"] = format!(
                    "Repair succeeded; old installation backup remains: {}",
                    error.message
                )
                .into();
            }
        }
        self.changed("progress", None);
    }
}

pub(super) fn prepare(
    root: &Path,
    version: &str,
    archive: &Path,
    cancel: &Cancel,
) -> Result<PathBuf, ara_update::Error> {
    fs::create_dir_all(root)?;
    let staged = root.join(format!(".repair-{version}"));
    ara_update::extract(archive, &staged, cancel)?;
    let result = (|| {
        validate_payload(&staged, version)?;
        ara_update::write_json(
            &staged.join(".ara-managed.json"),
            &json!({"version":version}),
        )?;
        cancel.check()?;
        Ok(staged.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staged);
    }
    result
}

fn failure(message: impl Into<String>) -> ara_update::Error {
    ara_update::Error {
        code: ara_update::IO,
        message: message.into(),
    }
}
fn update_error(error: RpcError) -> ara_update::Error {
    failure(error.message)
}
