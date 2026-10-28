//! Uninstall prepares off-thread, commits registration atomically, then removes files off-thread.
use super::*;

impl Hub {
    pub(in crate::service) fn engine_removal_running(&self, version: &str) -> bool {
        self.updates
            .jobs
            .values()
            .any(|job| job.kind == "uninstall" && job.version == version && job.state == "running")
    }

    pub(super) fn begin_uninstall(&mut self, version: String) -> Result<Value, RpcError> {
        ara_update::version(&version).map_err(rpc)?;
        let id = format!("uninstall-{version}");
        if let Some(job) = self.updates.jobs.get(&id)
            && job.state == "running"
        {
            return Ok(json!(job));
        }
        let root = self.updates.config.engines.join(&version);
        let trash = root.with_file_name(format!(".uninstall-{version}"));
        // A failed cleanup can be retried even after the registration was removed.
        let cleanup = !self
            .registry
            .engines
            .iter()
            .any(|engine| engine.version == version);
        if cleanup {
            validate_managed(&trash, &version)?;
        } else {
            let engine = self.registry.engine(&version)?;
            if !same_executable(engine, &root) {
                return Err(invalid(
                    "only Hub-managed installations can be uninstalled; remove registration for external builds",
                ));
            }
            validate_managed(&root, &version)?;
            self.ensure_unused(&version, &root)?;
            if trash.exists() {
                return Err(invalid(format!(
                    "unfinished uninstall at {}; retry its cleanup before uninstalling another copy",
                    trash.display()
                )));
            }
        }
        let cancel = Cancel::default();
        let flag = cancel.clone();
        let tx = self.updates.tx.clone();
        let worker_id = id.clone();
        let job = Job {
            id: id.clone(),
            kind: "uninstall".into(),
            version,
            state: "running".into(),
            phase: if cleanup { "removing" } else { "preparing" }.into(),
            cancellable: !cleanup,
            disk: None,
            progress: Progress {
                received: 0,
                total: 0,
            },
            result: None,
            error: None,
            cancel,
            proof: None,
            repair_source: None,
        };
        std::thread::Builder::new()
            .name(format!("ara-{id}"))
            .spawn(move || {
                background_priority();
                if cleanup {
                    let result = remove_files(&trash).map(|()| trash).map_err(Into::into);
                    let _ = tx.send(Message::Uninstalled(worker_id, result));
                } else {
                    let result = inspect_files(&root, &flag).map(|()| root);
                    let _ = tx.send(Message::UninstallPrepared(worker_id, result));
                }
            })
            .map_err(io_error)?;
        self.updates.jobs.insert(id, job.clone());
        self.changed("progress", None);
        Ok(json!(job))
    }

    pub(super) fn ensure_unused(&self, version: &str, root: &Path) -> Result<(), RpcError> {
        if self.registry.projects.iter().any(|project| {
            project.engine_version == version && self.status_of(project)["state"] == "open"
        }) {
            return Err(invalid(
                "close projects using this engine before uninstalling",
            ));
        }
        let executable = fs::canonicalize(&self.config.engine.executable)
            .unwrap_or_else(|_| dunce::simplified(&self.config.engine.executable).to_path_buf());
        let root = fs::canonicalize(root).unwrap_or_else(|_| dunce::simplified(root).to_path_buf());
        if executable.starts_with(&root) {
            return Err(invalid(
                "this engine hosts the running Hub service; restart the service from the standalone Hub before uninstalling",
            ));
        }
        Ok(())
    }

    pub(super) fn commit_uninstall(
        &mut self,
        id: String,
        result: Result<PathBuf, ara_update::Error>,
    ) {
        let Some(job) = self.updates.jobs.get(&id) else {
            return;
        };
        let version = job.version.clone();
        let cancellation = job.cancel.check();
        let result = result.and_then(|root| {
            cancellation?;
            self.ensure_unused(&version, &root).map_err(update_error)?;
            validate_managed(&root, &version).map_err(update_error)?;
            let trash = root.with_file_name(format!(".uninstall-{version}"));
            fs::rename(&root, &trash)?;
            let previous = self.registry.engines.clone();
            self.registry
                .engines
                .retain(|engine| engine.version != version);
            if let Err(error) = self.registry.save(&self.config.directory) {
                self.registry.engines = previous;
                fs::rename(&trash, &root)?;
                return Err(update_error(error));
            }
            Ok(trash)
        });
        match result {
            Err(error) => self.finish_uninstall(id, Err(error)),
            Ok(trash) => {
                if let Some(job) = self.updates.jobs.get_mut(&id) {
                    job.phase = "removing".into();
                    job.cancellable = false;
                    job.result = Some(json!({"removed":version}));
                }
                let tx = self.updates.tx.clone();
                let worker_id = id.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("ara-{id}-cleanup"))
                    .spawn(move || {
                        background_priority();
                        let result = remove_files(&trash).map(|()| trash).map_err(Into::into);
                        let _ = tx.send(Message::Uninstalled(worker_id, result));
                    });
                if let Err(error) = spawned {
                    self.finish_uninstall(id, Err(error.into()));
                }
                self.changed("engine", None);
                self.changed("progress", None);
            }
        }
    }

    pub(super) fn finish_uninstall(
        &mut self,
        id: String,
        result: Result<PathBuf, ara_update::Error>,
    ) {
        if let Some(job) = self.updates.jobs.get_mut(&id) {
            job.cancellable = false;
            match result {
                Ok(_) => {
                    job.state = "complete".into();
                    job.result = Some(json!({"removed":job.version}));
                }
                Err(error) => {
                    job.state = if error.code == ara_update::CANCELLED {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    job.error = Some(error.diagnostic());
                }
            }
            job.phase = job.state.clone();
        }
        self.changed("progress", None);
    }
}

fn update_error(error: RpcError) -> ara_update::Error {
    ara_update::Error {
        code: ara_update::IO,
        message: error.message,
    }
}

pub(super) fn validate_managed(root: &Path, version: &str) -> Result<(), RpcError> {
    let metadata = fs::symlink_metadata(root).map_err(io_error)?;
    if linked(&metadata) || !metadata.is_dir() {
        return Err(invalid(
            "refusing to inspect or uninstall a linked or invalid installation directory",
        ));
    }
    let ownership = root.join(".ara-managed.json");
    let metadata = fs::symlink_metadata(&ownership).map_err(io_error)?;
    if linked(&metadata) || !metadata.is_file() {
        return Err(invalid(
            "managed installation ownership proof must be a local regular file",
        ));
    }
    let marker: Value =
        serde_json::from_slice(&fs::read(ownership).map_err(io_error)?).map_err(io_error)?;
    if marker["version"] != version {
        return Err(invalid(
            "only Hub-managed installations with a matching version can be uninstalled",
        ));
    }
    Ok(())
}

pub(super) fn linked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT includes junctions and other reparse tags,
        // not only the symbolic-link tag exposed by FileType::is_symlink.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn inspect_files(root: &Path, cancel: &Cancel) -> Result<(), ara_update::Error> {
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        cancel.check()?;
        for entry in fs::read_dir(directory)? {
            cancel.check()?;
            let entry = entry?;
            // Never follow links outside the managed installation.
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
            }
        }
    }
    cancel.check()
}

// Keep ownership proof until cleanup succeeds, so a locked file cannot make retry
// impossible after other files have already been removed.
fn remove_files(root: &Path) -> io::Result<()> {
    let marker = root.join(".ara-managed.json");
    let proof = fs::read(&marker)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.path() == marker {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    fs::remove_file(&marker)?;
    if let Err(error) = fs::remove_dir(root) {
        fs::write(marker, proof)?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn failed_cleanup_keeps_ownership_proof_for_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = ara_update::testing::scratch("uninstall-locked");
        let proof = br#"{"version":"1.0.0"}"#;
        fs::write(root.join(".ara-managed.json"), proof).expect("ownership proof");
        let locked = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(root.join("locked.exe"))
            .expect("locked image");
        assert!(remove_files(&root).is_err());
        assert_eq!(
            fs::read(root.join(".ara-managed.json")).expect("proof retained"),
            proof
        );
        drop(locked);
        remove_files(&root).expect("retry");
        assert!(!root.exists());
    }

    #[test]
    fn cancellation_during_preparation_preserves_the_entire_installation() {
        let root = ara_update::testing::scratch("uninstall-cancel");
        fs::write(root.join("ara.exe"), b"engine").expect("engine fixture");
        let cancel = Cancel::default();
        cancel.cancel();
        assert_eq!(
            inspect_files(&root, &cancel).expect_err("cancelled").code,
            ara_update::CANCELLED
        );
        assert_eq!(
            fs::read(root.join("ara.exe")).expect("preserved engine"),
            b"engine"
        );
    }
}
