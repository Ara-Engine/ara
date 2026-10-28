//! A patch batch and its launcher belong to one live DX server, never the next run.
use crate::binaries::Binaries;
use anyhow::{Context, Result};
use std::{fs, path::Path};

pub(super) fn restore_template(root: &Path) -> Result<()> {
    restore_at(&crate::lab::directory(root).join("binaries.json"))
}

fn restore_at(cache: &Path) -> Result<()> {
    let Ok(bytes) = fs::read(cache) else {
        return Ok(());
    };
    let Ok(mut binaries) = serde_json::from_slice::<Binaries>(&bytes) else {
        return Ok(());
    };
    let launcher = binaries.get("ara-player").is_some_and(|path| {
        path.file_stem()
            .is_some_and(|name| name == "ara-hotpatch-lab-player")
    });
    if !launcher && !binaries.contains_key("ara-hotpatch-batch") {
        return Ok(());
    }
    let template = binaries
        .get("ara-player-template")
        .filter(|path| path.is_file())
        .cloned();
    if let Some(template) = template {
        binaries.insert("ara-player".into(), template);
        binaries.remove("ara-hotpatch-batch");
        fs::write(cache, serde_json::to_vec(&binaries)?)
            .context("restore normal lab template after the DX server lifetime")?;
    } else {
        // Only the disposable binary receipt is invalidated; scene selection survives.
        fs::remove_file(cache).context("discard stale lab launcher receipt")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_new_run_uses_the_normal_template_without_replaying_an_old_pid_batch() {
        let scratch = tempfile::tempdir().unwrap();
        let cache = scratch.path().join("binaries.json");
        let template = scratch.path().join("ara-player.exe");
        fs::write(&template, "normal player").unwrap();
        let binaries = Binaries::from([
            (
                "ara-player".into(),
                scratch.path().join("ara-hotpatch-lab-player.exe"),
            ),
            ("ara-player-template".into(), template.clone()),
            (
                "ara-hotpatch-batch".into(),
                scratch.path().join("old-pid-batch.json"),
            ),
        ]);
        fs::write(&cache, serde_json::to_vec(&binaries).unwrap()).unwrap();
        restore_at(&cache).unwrap();
        let restored: Binaries = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
        assert_eq!(restored.get("ara-player"), Some(&template));
        assert!(!restored.contains_key("ara-hotpatch-batch"));
        restore_at(&cache).unwrap();
    }
    #[test]
    fn missing_templates_invalidate_only_the_disposable_binary_receipt() {
        let scratch = tempfile::tempdir().unwrap();
        let cache = scratch.path().join("binaries.json");
        let selection = scratch.path().join("selections.json");
        fs::write(&selection, "saved scene").unwrap();
        fs::write(
            &cache,
            serde_json::to_vec(&Binaries::from([(
                "ara-player".into(),
                scratch.path().join("ara-hotpatch-lab-player.exe"),
            )]))
            .unwrap(),
        )
        .unwrap();
        restore_at(&cache).unwrap();
        assert!(!cache.exists());
        assert_eq!(fs::read_to_string(selection).unwrap(), "saved scene");
    }
}
