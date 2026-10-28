//! Real Host Undo/Redo with controlled scanner publications reproduces the native cache race.
use super::*;
use crate::shell::assets::scan_fixture::{Driver, controlled};
use ara_ui::{input::InputEvent, source::stable_id};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Project(PathBuf);
impl Project {
    fn new() -> ara_ui::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())?
            .join("asset-history-index")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("game/levels"))?;
        std::fs::create_dir_all(root.join("game/Art"))?;
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Index barrier","layoutVersion":3,"main":"levels/main.scene"}),
            ),
            (
                "game/levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_indexbarrier","records":{"e_context":{"name":"Context","Transform":{}}}}),
            ),
            (
                "game/Art/A.prefab",
                json!({"$schema":"ara://schema/prefab/1","id":"pf_indexa","records":{"e_originala":{"name":"A","Transform":{}}}}),
            ),
            (
                "game/Art/B.prefab",
                json!({"$schema":"ara://schema/prefab/1","id":"pf_indexb","records":{"e_originalb":{"name":"B","Transform":{}}}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value))?;
        }
        Ok(Self(root))
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        if let Ok(parent) = std::path::absolute(crate::test_target())
            && self.0.starts_with(parent.join("asset-history-index"))
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
fn rows(copied: bool) -> Vec<assets::Asset> {
    let mut items = vec![assets::Asset::folder("game/Art".into(), Some("Art".into()))];
    for name in if copied {
        vec!["A", "B", "A Copy", "B Copy"]
    } else {
        vec!["A", "B"]
    } {
        let mut asset =
            assets::Asset::unindexed(format!("game/Art/{name}.prefab"), "prefab".into());
        asset.authored_path = Some(format!("Art/{name}.prefab"));
        asset.document_kind = Some("prefab".into());
        items.push(asset);
    }
    items
}
fn select(editor: &mut Editor, shell: &mut Shell, copied: bool) -> ara_ui::Result<Selection> {
    let paths = if copied {
        ["game/Art/A Copy.prefab", "game/Art/B Copy.prefab"]
    } else {
        ["game/Art/A.prefab", "game/Art/B.prefab"]
    };
    let primary = if copied { paths[0] } else { paths[1] };
    editor.action(
        "editor.workspace",
        json!({"folder":"game/Art","asset":primary}),
    )?;
    shell.asset_selection.set_selected(paths.map(stable_id));
    shell.asset_selection.focus(stable_id(primary));
    shell.asset_selection_path = Some(primary.into());
    Ok(Selection::read(shell, editor))
}
fn exact(editor: &Editor, shell: &mut Shell, expected: &Selection) {
    shell.asset_selection_order(editor);
    let actual = Selection::read(shell, editor);
    assert_eq!(&actual, expected);
    assert!(
        actual
            .asset
            .as_ref()
            .is_some_and(|primary| actual.keys.contains(&stable_id(primary))),
        "Inspector primary must remain in the exact multi-selection: {actual:?}"
    );
}
struct Fixture {
    editor: Editor,
    shell: Shell,
    worker: Driver,
    before: Selection,
    after: Selection,
    source: TxnId,
    revision: u64,
    _project: Project,
}
fn fixture() -> ara_ui::Result<Fixture> {
    let project = Project::new()?;
    let mut editor = Editor::open(&project.0)?;
    editor.action("editor.select", json!({"entities":["e_context"]}))?;
    editor.action("editor.layout.show", json!({"panel":"assets"}))?;
    let mut shell = Shell::new(&mut editor, [1440., 900.])?;
    let (browser, mut worker) = controlled("game");
    shell.browser = browser;
    let generation = worker.start();
    worker.publish(generation, rows(false))?;
    assert!(shell.browser.poll());
    let before = select(&mut editor, &mut shell, false)?;
    editor.action("cmd.apply", json!({"doc":"ara.project.json","why":"Create isolated duplicate history targets","commands":[
        {"cmd":"doc.create","args":{"path":"Art/A Copy.prefab","kind":"prefab","id":"pf_indexcopya"}},
        {"cmd":"doc.create","args":{"path":"Art/B Copy.prefab","kind":"prefab","id":"pf_indexcopyb"}}
    ]}))?;
    editor.host.flush();
    assert!(!editor.host.unsaved());
    let transaction = editor
        .host
        .store()
        .history()
        .last()
        .ok_or("Typed duplicate fixture omitted its transaction")?;
    let source = transaction.id.clone();
    let revision = transaction.result;
    let generation = worker.start();
    worker.publish(generation, rows(true))?;
    assert!(shell.browser.poll());
    let after = select(&mut editor, &mut shell, true)?;
    shell.rebuild(&mut editor)?;
    Ok(Fixture {
        _project: project,
        editor,
        shell,
        worker,
        before,
        after,
        source,
        revision,
    })
}
fn retry(editor: &mut Editor, shell: &mut Shell, worker: &mut Driver) -> ara_ui::Result<()> {
    while worker.requested() {}
    shell.rebuild(editor)?;
    shell.frame()?;
    let index = shell
        .shown_node("assets-refresh")
        .ok_or("Pending index has no Refresh Assets control")?;
    let rect = shell.ui.nodes[index].rect;
    let clip = shell.ui.nodes[index].clip;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    assert!(clip.contains(point[0], point[1]));
    assert!(
        !shell.ui.nodes[index].state.disabled,
        "Retry must stay reachable while an asset operation is busy"
    );
    shell.event(InputEvent::PointerDown(point), &mut Clip)?;
    shell.event(InputEvent::PointerUp(point), &mut Clip)?;
    shell.dispatch(editor)?;
    assert!(
        worker.requested(),
        "The native retry must wake the existing scanner"
    );
    Ok(())
}
fn prepared(f: &mut Fixture) -> ara_ui::Result<()> {
    // Only the UI receipt is seeded; source points at the real typed fixture
    // transaction. Native acceptance owns asynchronous worker execution.
    select(&mut f.editor, &mut f.shell, false)?;
    let changes = rows(false)
        .into_iter()
        .filter(|asset| asset.kind != "folder")
        .map(|asset| {
            let destination = if asset.path.ends_with("/A.prefab") {
                "Art/A Copy.prefab"
            } else {
                "Art/B Copy.prefab"
            };
            (asset, Some(destination.into()))
        })
        .collect();
    f.shell.asset_ops.job = Some(Job {
        id: "jb_savedduplicate".into(),
        status: json!({"status":"done","result":{"txn":f.source.0},"message":"Asset duplicate complete"}),
        polled: Instant::now(),
        began: Instant::now() - Duration::from_secs(31),
        plan: plan::Plan::new("Duplicate assets", Vec::new(), Vec::new(), changes, true),
        selection: f.before.clone(),
        following: true,
        scan_barrier: None,
        scan_wait_notice: false,
        cancel_requested: false,
    });
    f.shell.asset_ops.revision += 1;
    Ok(())
}

#[test]
fn undo_redo_waits_for_post_save_snapshot_and_failed_scan_retry_keeps_exact_selection()
-> ara_ui::Result<()> {
    let mut f = fixture()?;
    f.shell.asset_ops.history.remember(
        f.source.clone(),
        f.revision,
        f.before.clone(),
        f.after.clone(),
    );
    let old = f.worker.start();
    f.editor.action("history.undo", json!({}))?;
    f.editor.host.flush();
    f.shell.observe_asset_history(&f.editor);
    assert!(!f.shell.follow_asset_history(&mut f.editor)?);
    f.worker.publish(old, rows(true))?; // queued pre-Undo snapshot
    assert!(!f.shell.browser.poll());
    assert!(!f.shell.follow_asset_history(&mut f.editor)?);
    exact(&f.editor, &mut f.shell, &f.after);
    let undo = f.worker.start();
    f.worker.publish(undo, rows(false))?;
    assert!(f.shell.browser.poll());
    assert!(f.shell.follow_asset_history(&mut f.editor)?);
    exact(&f.editor, &mut f.shell, &f.before);

    let undo_only = f.worker.start();
    f.editor.action("history.redo", json!({}))?;
    f.editor.host.flush();
    f.shell.observe_asset_history(&f.editor);
    assert!(!f.shell.follow_asset_history(&mut f.editor)?);
    f.worker.publish(undo_only, rows(false))?; // late Undo-era publication
    assert!(!f.shell.browser.poll());
    assert!(!f.shell.follow_asset_history(&mut f.editor)?);
    exact(&f.editor, &mut f.shell, &f.before);
    let failed = f.worker.start();
    f.worker.fail(failed)?;
    assert!(f.shell.browser.poll());
    assert!(f.shell.follow_asset_history(&mut f.editor)?);
    assert!(f.shell.asset_ops.history.pending());
    assert_eq!(f.shell.status_note.as_deref(), Some(HISTORY_SCAN_WAIT));
    exact(&f.editor, &mut f.shell, &f.before);
    let revision = f.editor.host.store().revision();
    retry(&mut f.editor, &mut f.shell, &mut f.worker)?;
    let redo = f.worker.start();
    f.worker.publish(redo, rows(true))?;
    assert!(f.shell.browser.poll());
    assert!(f.shell.follow_asset_history(&mut f.editor)?);
    assert!(!f.shell.asset_ops.history.pending());
    assert_ne!(f.shell.status_note.as_deref(), Some(HISTORY_SCAN_WAIT));
    exact(&f.editor, &mut f.shell, &f.after);
    assert_eq!(
        f.editor.host.store().revision(),
        revision,
        "Retry/read-model handoff must not author content"
    );
}

#[test]
fn composed_rapid_reverts_reset_the_barrier_and_equal_fresh_rows_complete_it() -> ara_ui::Result<()>
{
    let mut f = fixture()?;
    f.shell.asset_ops.history.remember(
        f.source.clone(),
        f.revision,
        f.before.clone(),
        f.after.clone(),
    );
    let pre_undo = f.worker.start();
    f.editor.action("history.undo", json!({}))?;
    f.editor.host.flush();
    f.shell.observe_asset_history(&f.editor);
    f.shell.follow_asset_history(&mut f.editor)?;
    let previous = f
        .shell
        .asset_ops
        .history
        .pending
        .as_ref()
        .and_then(|pending| pending.scan_barrier);
    f.worker.publish(pre_undo, rows(true))?;
    let undo_only = f.worker.start();
    f.editor.action("history.redo", json!({}))?;
    f.editor.host.flush();
    f.shell.observe_asset_history(&f.editor);
    assert!(
        f.shell
            .asset_ops
            .history
            .pending
            .as_ref()
            .is_some_and(|pending| pending.scan_barrier.is_none())
    );
    f.shell.follow_asset_history(&mut f.editor)?;
    assert_ne!(
        f.shell
            .asset_ops
            .history
            .pending
            .as_ref()
            .and_then(|pending| pending.scan_barrier),
        previous
    );
    assert!(!f.shell.browser.poll());
    assert!(!f.shell.follow_asset_history(&mut f.editor)?);
    f.worker.publish(undo_only, rows(false))?;
    assert!(f.shell.browser.poll());
    assert!(!f.shell.follow_asset_history(&mut f.editor)?);
    exact(&f.editor, &mut f.shell, &f.after);
    let fresh = f.worker.start();
    f.worker.publish(fresh, rows(true))?;
    assert!(f.shell.browser.poll());
    assert!(f.shell.follow_asset_history(&mut f.editor)?);
    exact(&f.editor, &mut f.shell, &f.after);
    let barrier = f.shell.browser.refresh_after_current_scan();
    let equal = f.worker.start();
    f.worker.publish(equal, rows(true))?;
    assert!(!f.shell.browser.poll());
    assert!(f.shell.browser.completed_scan_after(barrier));
    Ok(())
}

#[test]
fn prepared_job_rejects_pre_receipt_rows_and_keeps_selection_through_timeout_and_retry()
-> ara_ui::Result<()> {
    let mut f = fixture()?;
    prepared(&mut f)?;
    let old = f.worker.start();
    let revision = f.editor.host.store().revision();
    f.shell.poll_asset_operations(&mut f.editor)?;
    exact(&f.editor, &mut f.shell, &f.before);
    f.worker.publish(old, rows(true))?;
    assert!(!f.shell.browser.poll());
    f.shell.poll_asset_operations(&mut f.editor)?;
    assert!(
        f.shell
            .asset_ops
            .job
            .as_ref()
            .is_some_and(|job| job.following)
    );
    assert_eq!(f.shell.status_note.as_deref(), Some(JOB_SCAN_WAIT));
    exact(&f.editor, &mut f.shell, &f.before);
    let failed = f.worker.start();
    f.worker.fail(failed)?;
    assert!(f.shell.browser.poll());
    f.shell.poll_asset_operations(&mut f.editor)?;
    assert!(
        f.shell
            .asset_ops
            .job
            .as_ref()
            .is_some_and(|job| job.following)
    );
    exact(&f.editor, &mut f.shell, &f.before);
    retry(&mut f.editor, &mut f.shell, &mut f.worker)?;
    let fresh = f.worker.start();
    f.worker.publish(fresh, rows(true))?;
    assert!(f.shell.browser.poll());
    f.shell.poll_asset_operations(&mut f.editor)?;
    assert!(
        f.shell
            .asset_ops
            .job
            .as_ref()
            .is_some_and(|job| !job.following)
    );
    assert_ne!(f.shell.status_note.as_deref(), Some(JOB_SCAN_WAIT));
    exact(&f.editor, &mut f.shell, &f.after);
    assert_eq!(f.editor.host.store().revision(), revision);
    Ok(())
}

#[test]
fn fresh_job_snapshot_does_not_replace_a_later_human_selection() -> ara_ui::Result<()> {
    let mut f = fixture()?;
    prepared(&mut f)?;
    let old = f.worker.start();
    f.shell.poll_asset_operations(&mut f.editor)?;
    f.worker.publish(old, rows(true))?;
    f.shell.browser.poll();
    let asset = f
        .shell
        .browser
        .items
        .iter()
        .find(|asset| asset.path == "game/Art/A.prefab")
        .cloned()
        .ok_or("Source asset missing")?;
    f.shell.select_asset_item(&mut f.editor, &asset)?;
    let human = Selection::read(&f.shell, &f.editor);
    let fresh = f.worker.start();
    f.worker.publish(fresh, rows(true))?;
    f.shell.browser.poll();
    f.shell.poll_asset_operations(&mut f.editor)?;
    assert!(
        f.shell
            .asset_ops
            .job
            .as_ref()
            .is_some_and(|job| !job.following)
    );
    exact(&f.editor, &mut f.shell, &human);
    Ok(())
}
