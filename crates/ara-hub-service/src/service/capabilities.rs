//! Engine declarations are queried without opening a project or loading its scripts.
use super::*;
use ara_diag::{Diagnostic, codes};
use std::{io::Read, time::SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    length: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Declaration {
    engine_version: String,
    project_layouts: Vec<u64>,
}

#[derive(Default)]
pub(super) struct Capabilities(BTreeMap<PathBuf, (Identity, Vec<u64>)>);

impl Capabilities {
    fn layouts(&mut self, executable: &Path, directory: &Path) -> Vec<u64> {
        let Ok(path) = fs::canonicalize(executable) else {
            return vec![3];
        };
        let path = crate::normalized_path(&path).to_owned();
        let Ok(metadata) = fs::metadata(&path) else {
            return vec![3];
        };
        let identity = Identity {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        };
        if let Some((previous, layouts)) = self.0.get(&path)
            && previous == &identity
        {
            return layouts.clone();
        }
        let layouts = query(&path, directory).unwrap_or_else(|| vec![3]);
        self.0.insert(path, (identity, layouts.clone()));
        layouts
    }
}

fn declaration(bytes: &[u8]) -> Option<Vec<u64>> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let declaration: Declaration = serde_json::from_slice(bytes).ok()?;
    (!declaration.engine_version.trim().is_empty()
        && !declaration.project_layouts.is_empty()
        && declaration
            .project_layouts
            .iter()
            .all(|version| *version > 0))
    .then_some(declaration.project_layouts)
}

/// Bound both execution time and output; only this exact child is terminated on timeout.
pub(super) fn query(executable: &Path, directory: &Path) -> Option<Vec<u64>> {
    const LIMIT: u64 = 64 * 1024;
    let mut command = Command::new(executable);
    command
        .args(["--json", "capabilities"])
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hidden(&mut command);
    let mut child = command.spawn().ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let reader = std::thread::Builder::new()
        .name("ara-engine-capabilities".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout.take(LIMIT + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() <= LIMIT as usize).then_some(bytes)
        });
    let Ok(reader) = reader else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    // A badly behaved CLI can give its stdout handle to a descendant. Never wait
    // for that handle after our child exits: the bounded reader owns no project state.
    while !reader.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    if !success || !reader.is_finished() {
        return None;
    }
    declaration(&reader.join().ok()??)
}

impl Hub {
    /// The marker chooses the engine. The Hub only delegates; it never loads old data.
    pub(super) fn project_engine(
        &mut self,
        project: &ProjectEntry,
    ) -> Result<EngineBuild, RpcError> {
        let marker_path = project.path.join("ara.project.json");
        let marker: Value =
            serde_json::from_slice(&fs::read(&marker_path).map_err(io_error)?).map_err(io_error)?;
        let version = marker["engineVersion"]
            .as_str()
            .ok_or_else(|| invalid("project is missing its engineVersion pin"))?;
        let engine = self.registry.engine(version)?.clone();
        let layouts = self
            .capabilities
            .layouts(&engine.executable, &self.config.directory);
        if ara_doc::detect_project_layout(&marker).is_current() {
            if !layouts.contains(&4) {
                return Err(Diagnostic::error(
                    codes::PROJECT_ENGINE_LAYOUT_UNSUPPORTED,
                    "This project uses the `content/` layout and needs Ara preview 4 or newer",
                )
                .file(marker_path.to_string_lossy())
                .into());
            }
        } else if layouts.contains(&4) || !layouts.contains(&3) {
            ara_host::project_layout::require_current_layout(&project.path)?;
        }
        Ok(engine)
    }

    pub(super) fn current_engine(&mut self, headless: bool) -> Value {
        let version = ara_host::project_layout::CURRENT_ENGINE_VERSION;
        let registered = self.registry.engine(version).is_ok();
        let available = self.registry.engine(version).is_ok_and(|engine| {
            (headless
                || engine
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.is_file()))
                && self
                    .capabilities
                    .layouts(&engine.executable, &self.config.directory)
                    .contains(&4)
        });
        json!({"engineVersion":version,"available":available,"registered":registered})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_complete_declaration_overrides_preview_three() {
        for bytes in [
            b"".as_slice(),
            b"{}",
            br#"{"projectLayouts":[4]}"#,
            br#"{"engineVersion":"4","projectLayouts":[4,"3"]}"#,
            br#"{"engineVersion":"","projectLayouts":[4]}"#,
            br#"{"engineVersion":"4","projectLayouts":[]}"#,
        ] {
            assert_eq!(declaration(bytes), None);
        }
        assert_eq!(
            declaration(
                b"\xef\xbb\xbf {\"engineVersion\":\"0.0.1-alpha\",\"projectLayouts\":[4]} \n"
            ),
            Some(vec![4])
        );
    }
}
