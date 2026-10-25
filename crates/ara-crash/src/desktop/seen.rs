//! Acknowledgements describe reports, rather than depending on marker-file mtime.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct Report {
    name: PathBuf,
    modified: SystemTime,
    bytes: u64,
}

pub(super) fn acknowledge(
    directory: &Path,
    application: &str,
    interactive: bool,
    offer: impl FnOnce(&Path),
) -> Result<()> {
    // Automation must leave the next human launch's notification untouched.
    if !interactive {
        return Ok(());
    }
    let marker = directory.join(format!(".{application}-seen"));
    let previous: BTreeSet<Report> = fs::read(&marker)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut reports = BTreeSet::new();
    for entry in fs::read_dir(directory)?.filter_map(Result::ok) {
        if !matches!(
            entry.path().extension().and_then(|e| e.to_str()),
            Some("json" | "dmp" | "managed")
        ) {
            continue;
        }
        if let Ok(metadata) = entry.metadata()
            && metadata.is_file()
            && let Ok(modified) = metadata.modified()
        {
            reports.insert(Report {
                name: entry.file_name().into(),
                modified,
                bytes: metadata.len(),
            });
        }
    }
    let pending = reports.iter().any(|report| !previous.contains(report));
    // Persist before the modal: another launch must not re-offer even if the
    // process fails or is killed while the dialog is open.
    fs::write(
        marker,
        ara_json::to_canonical_string(&serde_json::to_value(&reports)?),
    )?;
    if pending {
        offer(directory);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledgements_survive_stale_marker_times_and_precede_the_dialog() {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/crash-seen-tests");
        fs::create_dir_all(&target).expect("target");
        let directory = tempfile::tempdir_in(target).expect("isolated reports");
        let root = directory.path();
        let marker = root.join(".ara-editor-seen");
        fs::write(&marker, []).expect("legacy empty marker");
        fs::write(root.join("first.json"), "{}").expect("report");
        acknowledge(root, "ara-editor", false, |_| panic!("headless dialog")).expect("headless");
        assert_eq!(fs::read(&marker).expect("legacy marker"), b"");
        acknowledge(root, "ara-editor", true, |_| {
            let seen: BTreeSet<Report> =
                serde_json::from_slice(&fs::read(&marker).expect("persisted before dialog"))
                    .expect("explicit acknowledgement");
            assert_eq!(seen.len(), 1);
        })
        .expect("first launch");
        fs::File::options()
            .write(true)
            .open(&marker)
            .expect("marker")
            .set_modified(SystemTime::UNIX_EPOCH)
            .expect("simulate stale Windows LastWriteTime");
        acknowledge(root, "ara-editor", true, |_| panic!("already offered"))
            .expect("second launch");
        fs::write(root.join("second.dmp"), "dump").expect("next report");
        let same_time = fs::metadata(root.join("first.json"))
            .expect("report")
            .modified()
            .expect("mtime");
        fs::File::options()
            .write(true)
            .open(root.join("second.dmp"))
            .expect("dump")
            .set_modified(same_time)
            .expect("timestamp tie");
        let mut offered = false;
        acknowledge(root, "ara-editor", true, |_| offered = true).expect("new report");
        assert!(offered, "a new name at the same timestamp is a new report");
        acknowledge(root, "ara-editor", true, |_| panic!("repeated report")).expect("third launch");
    }
}
