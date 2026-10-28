//! Short Assets drawers retain file interaction while every background action scrolls into view.
use super::*;
use ara_ui::input::InputEvent;
use ara_ui::source::stable_id;

struct Clipboard;
impl ara_ui_text::Clipboard for Clipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

fn receipt(shell: &mut Shell, editor: &Editor, id: &str, running: bool) {
    shell.asset_import.jobs.push(Job {
        id: id.into(),
        destination: "Imported content with a descriptive destination".into(),
        sources: 2,
        status: backend::Status::read(if running {
            json!({"status":"running","cancellable":true,"progress":0.5,"message":"Importing the next selected asset while preserving the existing browser selection"})
        } else {
            json!({"status":"done","result":{"created":[],"replaced":[],"skipped":[],"renamed":[]}})
        }),
        selection: Selection::read(shell, editor),
        last: Instant::now(),
        cancel_requested: false,
        preparation: None,
        plan: None,
        error: None,
    });
    shell.asset_import.revision += 1;
}

fn native_click(shell: &mut Shell, id: &str) -> ara_ui::Result<()> {
    let index = shell.shown_node(id).ok_or("Visible action missing")?;
    let node = &shell.ui.nodes[index];
    let visible = node.rect.intersection(node.clip);
    assert!(
        node.rect.w > 16.
            && node.rect.h >= 21.
            && visible.w >= node.rect.w - 0.5
            && visible.h >= node.rect.h - 0.5,
        "{id} needs a complete usable target: rect={:?}, clip={:?}",
        node.rect,
        node.clip
    );
    assert!(
        !node.state.disabled,
        "{id} must preserve its authority-live action"
    );
    let point = [visible.x + visible.w * 0.5, visible.y + visible.h * 0.5];
    let mut hit = shell.ui.hit_test(point[0], point[1]);
    while let Some(current) = hit {
        if current == index {
            break;
        }
        hit = shell.ui.nodes[current].element.parent;
    }
    assert_eq!(hit, Some(index), "{id} is obstructed");
    assert!(
        shell.ui.scrollbar_at(point).is_none(),
        "{id} lies under the scroll track"
    );
    shell.event(InputEvent::PointerDown(point), &mut Clipboard)?;
    shell.event(InputEvent::PointerUp(point), &mut Clipboard)?;
    assert!(
        shell
            .ui
            .drain_actions()
            .any(|action| action.node == stable_id(id)),
        "{id} did not emit its retained action"
    );
    Ok(())
}

fn wheel_to_action(shell: &mut Shell, id: &str) -> ara_ui::Result<()> {
    for _ in 0..64 {
        let index = shell.shown_node(id).ok_or("Job action missing")?;
        let node = &shell.ui.nodes[index];
        let visible = node.rect.intersection(node.clip);
        if visible.w >= node.rect.w - 0.5 && visible.h >= node.rect.h - 0.5 {
            return native_click(shell, id);
        }
        let delta = if node.rect.y < node.clip.y {
            node.rect.y - node.clip.y
        } else {
            node.rect.y + node.rect.h - node.clip.y - node.clip.h
        }
        .clamp(-32., 32.);
        let rail = shell
            .shown_node("asset-jobs-scroll")
            .ok_or("Shared job rail missing")?;
        let viewport = shell.ui.nodes[rail]
            .rect
            .intersection(shell.ui.nodes[rail].clip);
        assert!(
            viewport.h >= 27.5,
            "Job rail has no usable scroll viewport: {viewport:?}"
        );
        shell.event(
            InputEvent::Wheel {
                position: [viewport.x + viewport.w * 0.5, viewport.y + viewport.h * 0.5],
                delta: [0., delta],
            },
            &mut Clipboard,
        )?;
        shell.frame()?;
    }
    Err(format!("{id} was unreachable through the shared job rail").into())
}

#[test]
fn combined_active_and_terminal_jobs_leave_a_full_asset_target_in_short_and_normal_drawers()
-> ara_ui::Result<()> {
    let (mut editor, mut shell) = super::layout_tests::open()?;
    editor.action(
        "editor.layout.move",
        json!({"panel":"assets","slot":"float","rect":[80,80,640,500]}),
    )?;
    editor.action(
        "editor.search",
        json!({"panel":"assets","query":"Rail proof"}),
    )?;
    let filename = "Rail proof with a descriptive imported asset name that needs truncation.wav";
    shell.browser.insert_path(filename, "wav");
    receipt(&mut shell, &editor, "jb_railold", false);
    receipt(&mut shell, &editor, "jb_raillive", true);
    shell.rebuild(&mut editor)?;
    let id = editor.state.floating.iter().find(|window| matches!(&window.root, ara_ui::docking::DockNode::Area {tabs,..} if tabs.iter().any(|panel| panel == "assets")))
        .ok_or("Floating Assets host missing")?.id.clone();
    assert!(shell.enter(&id));
    let body = shell
        .shown_node("body-assets")
        .ok_or("Assets body missing")?;
    let chrome = shell.ui.size[1] - shell.ui.nodes[body].rect.h;
    let revision = editor.host.store().revision();
    for grid in [true, false] {
        editor.action("editor.workspace", json!({"grid":grid}))?;
        for running in [false, true] {
            shell.seed_asset_operation_rail(&editor, running);
            for height in [205., 420.] {
                shell.resize_surface(&id, [640., height + chrome])?;
                // Reuse the real surface resize/rebuild path. The second mount
                // uses its measured browser height to choose normal/compact cards.
                shell.rebuild(&mut editor)?;
                shell.rebuild(&mut editor)?;
                shell.frame()?;
                let body = shell
                    .shown_node("body-assets")
                    .ok_or("Assets body missing")?;
                assert!((shell.ui.nodes[body].rect.h - height).abs() <= 0.5);
                let rail = shell
                    .shown_node("asset-jobs-scroll")
                    .ok_or("Shared job rail missing")?;
                let viewport = shell.ui.nodes[rail]
                    .rect
                    .intersection(shell.ui.nodes[rail].clip);
                assert!(
                    (27.5..=180.5).contains(&viewport.h),
                    "Unbounded shared rail: {viewport:?}"
                );
                let file = shell.ui.nodes.iter().enumerate().find(|(index, node)| {
                    shell.ui.shown(*index) && matches!(shell.bindings.get(&stable_id(&node.element.id)), Some(Binding::Asset(asset)) if asset.path == filename)
                }).map(|(_, node)| node.element.id.clone()).ok_or("Asset card/list row missing")?;
                native_click(&mut shell, &file)?;
                for node in &shell.ui.nodes {
                    if node.element.id.starts_with(&format!("{file}-name")) {
                        let visible = node.rect.intersection(node.clip);
                        assert!(
                            visible.h >= node.rect.h - 0.5,
                            "The short drawer must retain a complete asset name: {:?} in {:?}",
                            node.rect,
                            node.clip
                        );
                    }
                }
                wheel_to_action(
                    &mut shell,
                    if running {
                        "asset-operation-cancel"
                    } else {
                        "asset-operation-dismiss"
                    },
                )?;
                for job in ["jb_railold", "jb_raillive"] {
                    wheel_to_action(
                        &mut shell,
                        &format!("asset-import-{:016x}-action", stable_id(job)),
                    )?;
                }
                // Scrolling status never consumes or overlays the browser target.
                native_click(&mut shell, &file)?;
                assert_eq!(editor.host.store().revision(), revision);
            }
        }
    }
    Ok(())
}
