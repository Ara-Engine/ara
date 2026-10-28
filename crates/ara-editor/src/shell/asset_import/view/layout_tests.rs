//! Real Assets layout keeps authority-bound job controls visible and scroll-reachable.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn open() -> ara_ui::Result<(Editor, Shell)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::test_target().join("import-job-layout").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
    for (path, value) in [
        (
            "ara.project.json",
            json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Import controls","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_importlayout","records":{}}),
        ),
    ] {
        std::fs::write(
            root.join(
                if path == "ara.project.json" || path.starts_with("settings/") {
                    std::path::PathBuf::from(path)
                } else {
                    std::path::PathBuf::from(ara_doc::AUTHORING_ROOT).join(path)
                },
            ),
            ara_json::to_canonical_string(&value),
        )?;
    }
    let mut editor = Editor::open(&root)?;
    editor.action("editor.layout.show", json!({"panel":"assets"}))?;
    let shell = Shell::new(&mut editor, [1440., 900.])?;
    Ok((editor, shell))
}

#[test]
fn bounded_import_rail_exposes_cancel_and_dismiss_without_collapsing_the_browser()
-> ara_ui::Result<()> {
    let (mut editor, mut shell) = open()?;
    for (id, running, destination) in [
        ("jb_importlayoutold", true, "assets"),
        (
            "jb_importlayoutactive",
            true,
            "P4 Import Proof with a descriptive destination name",
        ),
    ] {
        // Only presentation state is seeded. Native acceptance supplies the real
        // worker; this regression isolates its retained geometry from worker timing.
        let selection = Selection::read(&shell, &editor);
        shell.asset_import.jobs.push(Job {
            id:id.into(),destination:destination.into(),sources:2,selection,
            status:backend::Status::read(if running {
                json!({"status":"running","cancellable":true,"progress":0.5,"message":"Importing native-cancel.wav"})
            }else {
                json!({"status":"done","cancellable":false,"result":{"created":[],"replaced":[],"skipped":[],"renamed":[]}})
            }),
            last:Instant::now(),cancel_requested:false,preparation:None,plan:None,error:None,
        });
    }
    shell.asset_import.revision += 1;
    shell.rebuild(&mut editor)?;
    shell.frame()?;
    let index = shell
        .shown_node("asset-jobs-scroll")
        .ok_or("Import rail is not mounted")?;
    let viewport = shell.ui.nodes[index].rect;
    assert!(
        (36. ..=180.).contains(&viewport.h),
        "Import rail has no bounded viewport: {viewport:?}"
    );
    let first = format!(
        "asset-import-{:016x}",
        ara_ui::source::stable_id("jb_importlayoutold")
    );
    for part in ["message", "progress"] {
        let index = shell
            .shown_node(&format!("{first}-{part}"))
            .ok_or("First import feedback missing")?;
        let node = &shell.ui.nodes[index];
        let visible = node.rect.intersection(node.clip);
        assert!(
            visible.h >= node.rect.h - 0.5,
            "The first running card's {part} must fit the intrinsic rail: {:?} in {:?}",
            node.rect,
            node.clip
        );
    }
    let browser = shell
        .shown_node("assets-content")
        .ok_or("Assets browser was removed")?;
    assert!(
        shell.ui.nodes[browser].rect.h > 32.,
        "Job progress consumed the browser's whole viewport"
    );
    let key = format!(
        "asset-import-{:016x}",
        ara_ui::source::stable_id("jb_importlayoutactive")
    );
    let title = shell
        .shown_node(&format!("{key}-title"))
        .ok_or("Job title missing")?;
    assert!(
        shell.ui.nodes[title].rect.w > 100.,
        "Job title was squeezed to intrinsic zero width"
    );
    assert!(
        shell.ui.nodes[title].rect.h <= 44.1,
        "A long title pushed its action out of reach"
    );
    let action = format!("{key}-action");
    // All cards stay reviewable in one ordinary scroll widget. Align the target
    // button using that widget, then test real clipping and hit geometry.
    let action_index = shell.shown_node(&action).ok_or("Cancel control missing")?;
    let top = shell.ui.nodes[action_index].rect.y - viewport.y;
    shell.ui.scroll_to(index, [0., top.max(0.)])?;
    shell.frame()?;
    let action_index = shell
        .shown_node(&action)
        .ok_or("Scrolled Cancel control missing")?;
    let node = &shell.ui.nodes[action_index];
    let visible = node.rect.intersection(node.clip);
    assert!(
        node.rect.w > 16. && visible.w >= node.rect.w * 0.99 && visible.h >= node.rect.h * 0.99,
        "Live Cancel is clipped: {:?} in {:?}",
        node.rect,
        node.clip
    );
    assert!(
        !node.state.disabled,
        "Authority-live cancellation must be enabled"
    );
    let at = [visible.x + visible.w * 0.5, visible.y + visible.h * 0.5];
    let mut hit = shell.ui.hit_test(at[0], at[1]);
    while let Some(index) = hit {
        if index == action_index {
            break;
        }
        hit = shell.ui.nodes[index].element.parent;
    }
    assert_eq!(
        hit,
        Some(action_index),
        "A visible overlay intercepts the import action"
    );
    if let Some(job) = shell.asset_import.jobs.last_mut() {
        job.status = backend::Status::read(
            json!({"status":"done","cancellable":false,"result":{"created":[],"replaced":[],"skipped":[],"renamed":[]}}),
        );
    }
    shell.asset_import.revision += 1;
    shell.rebuild(&mut editor)?;
    shell.frame()?;
    let label = shell
        .shown_node(&format!("{key}-action-label"))
        .ok_or("Completed Dismiss label missing")?;
    assert_eq!(
        shell.ui.nodes[label]
            .text
            .as_ref()
            .map(|text| text.value.as_str()),
        Some("Dismiss")
    );
    let button = shell
        .shown_node(&action)
        .ok_or("Completed Dismiss control missing")?;
    let button = &shell.ui.nodes[button];
    let visible = button.rect.intersection(button.clip);
    assert!(
        button.rect.w > 16.
            && visible.w >= button.rect.w * 0.99
            && visible.h >= button.rect.h * 0.99,
        "Completed Dismiss is clipped: {:?} in {:?}",
        button.rect,
        button.clip
    );
    assert!(!button.state.disabled);
    Ok(())
}
