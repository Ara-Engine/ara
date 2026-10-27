//! Read-only size inspection never enters the uninstall prepare/commit path.
use super::*;

#[derive(Serialize)]
pub(super) struct Inspection {
    version: String,
    destination: PathBuf,
    /// Sum of regular-file logical lengths; disk allocation/hardlinks may differ.
    bytes: u64,
    files: u64,
}

impl Hub {
    pub(super) fn begin_uninstall_preflight(&mut self, version: String) -> Result<Value, RpcError> {
        ara_update::version(&version).map_err(rpc)?;
        let id = format!("uninstall-preflight-{version}");
        if let Some(job) = self.updates.jobs.get(&id)
            && job.state == "running"
        {
            return Ok(json!(job));
        }
        let root = self.updates.config.engines.join(&version);
        let engine = self.registry.engine(&version)?;
        if !same_executable(engine, &root) {
            return Err(invalid(
                "only Hub-managed installations can be inspected for uninstall; remove registration for external builds",
            ));
        }
        uninstall::validate_managed(&root, &version)?;
        self.ensure_unused(&version, &root)?;
        if self.engine_removal_running(&version) {
            return Err(invalid(
                "wait for this version's uninstall to finish before inspecting it",
            ));
        }
        let cancel = Cancel::default();
        let flag = cancel.clone();
        let tx = self.updates.tx.clone();
        let worker_id = id.clone();
        let worker_version = version.clone();
        let job = Job {
            id: id.clone(),
            kind: "uninstall-preflight".into(),
            version,
            state: "running".into(),
            phase: "inspecting".into(),
            cancellable: true,
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
                let result = inspect(&root, &flag).map(|(bytes, files)| Inspection {
                    version: worker_version,
                    destination: root,
                    bytes,
                    files,
                });
                let _ = tx.send(Message::UninstallInspected(worker_id, result));
            })
            .map_err(io_error)?;
        self.updates.jobs.insert(id, job.clone());
        self.changed("progress", None);
        Ok(json!(job))
    }

    pub(super) fn finish_uninstall_preflight(
        &mut self,
        id: String,
        result: Result<Inspection, ara_update::Error>,
    ) {
        if let Some(job) = self.updates.jobs.get_mut(&id) {
            // Cancellation can arrive after the worker's final check and before
            // this authority consumes its message. It still wins that race.
            let result = job.cancel.check().and(result);
            job.cancellable = false;
            match result {
                Ok(inspection) => {
                    job.state = "complete".into();
                    job.result = Some(json!(inspection));
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

fn inspect(root: &Path, cancel: &Cancel) -> Result<(u64, u64), ara_update::Error> {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        cancel.check()?;
        let metadata = fs::symlink_metadata(&directory)?;
        if uninstall::linked(&metadata) || !metadata.is_dir() {
            return Err(inspection_error(&directory));
        }
        for entry in fs::read_dir(&directory)? {
            cancel.check()?;
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if uninstall::linked(&metadata) {
                return Err(inspection_error(&path));
            }
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| inspection_error(&path))?;
                files = files
                    .checked_add(1)
                    .ok_or_else(|| inspection_error(&path))?;
            } else {
                return Err(inspection_error(&path));
            }
        }
    }
    cancel.check()?;
    Ok((bytes, files))
}

fn inspection_error(path: &Path) -> ara_update::Error {
    ara_update::Error {
        code: ara_update::IO,
        message: format!(
            "cannot safely inspect installation entry {}: links, reparse points and special files are unsupported",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests;
