//! Compact trace evidence keeps exact semantic snapshots separate and unchanged.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        if let Ok(parent) = std::path::absolute(crate::test_target())
            && self.0.starts_with(parent.join("prefab-capture-documents"))
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn compact_capture_tracks_typed_changes_without_replacing_full_snapshot_assertions()
-> ara_ui::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let project = Project(
        std::path::absolute(crate::test_target())?
            .join("prefab-capture-documents")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )),
    );
    std::fs::create_dir_all(project.0.join("game/levels"))?;
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Capture documents","layoutVersion":3,"main":"levels/main.scene"}),
        ),
        (
            "game/levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_capturemain","records":{"e_capturemain":{"name":"Main","Transform":{}}}}),
        ),
        (
            "game/levels/untouched.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_captureuntouched","records":{"e_captureuntouched":{"name":"Untouched مرحبا ".repeat(2048),"Transform":{}}}}),
        ),
    ] {
        std::fs::write(project.0.join(path), ara_json::to_canonical_string(&value))?;
    }
    let mut editor = Editor::open(&project.0)?;
    // Explicitly load the second scene so the unrelated document participates in
    // the same all-workspace assertion which produced the large native report.
    editor.action("editor.scene.open", json!({"scene":"sc_captureuntouched"}))?;
    editor.action("editor.scene.open", json!({"scene":"sc_capturemain"}))?;
    let before = snapshot(&editor);
    let captured = capture_documents(&editor);
    let documents = before.as_object().ok_or("Missing full document snapshot")?;
    let manifest = captured
        .as_object()
        .ok_or("Missing capture document manifest")?;
    assert_eq!(documents.len(), manifest.len());
    for (path, body) in documents {
        let canonical = body
            .as_str()
            .ok_or("Full snapshot must retain document text")?;
        assert_eq!(manifest[path]["canonicalBytes"], json!(canonical.len()));
        assert_eq!(manifest[path]["blake3"].as_str().map(str::len), Some(64));
    }
    assert_eq!(
        snapshot(&editor),
        before,
        "Capture must not change authoring or exact snapshot data"
    );
    assert!(
        ara_json::to_canonical_string(&captured).len() < 2048,
        "Evidence size must depend on document count, not document bodies"
    );

    editor.apply(
        vec![command(
            "property.set",
            json!({"entity":"e_capturemain","path":"Transform.position","value":[3,2,1]}),
        )],
        "Change the relevant capture document",
        None,
    )?;
    let after = snapshot(&editor);
    let next = capture_documents(&editor);
    assert_ne!(
        after, before,
        "Full semantic assertions still see the typed change"
    );
    assert_ne!(
        next["game/levels/main.scene"]["blake3"],
        captured["game/levels/main.scene"]["blake3"]
    );
    assert_eq!(
        next["game/levels/untouched.scene"],
        captured["game/levels/untouched.scene"]
    );
    assert_eq!(
        after["game/levels/untouched.scene"],
        before["game/levels/untouched.scene"]
    );
    assert_eq!(next["ara.project.json"], captured["ara.project.json"]);
    Ok(())
}
