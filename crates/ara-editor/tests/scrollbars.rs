//! Hierarchy interactions exercise the full model through a bounded retained row pool.
use ara_editor::{Editor, shell::Shell};
use ara_ui::{Result, input::InputEvent, source::stable_id};
use ara_ui_text::Clipboard;
use serde_json::json;
use std::path::PathBuf;

struct Project(PathBuf);
impl Project {
    fn new() -> Result<Self> {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("editor-scroll-{}", std::process::id()));
        std::fs::create_dir_all(root.join("levels"))?;
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"name":"Scroll verification","main":"levels/main.scene"}),
            ),
        )?;
        let records: serde_json::Map<_, _> = (0..512)
            .map(|i| {
                (
                    format!("e_{i:08}"),
                    json!({"name":format!("Prop {i:03}"),"Transform":{}}),
                )
            })
            .collect();
        std::fs::write(
            root.join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_scroll01","records":records}),
            ),
        )?;
        Ok(Self(root))
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(self.0.starts_with(env!("CARGO_TARGET_TMPDIR")));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct HierarchyPool {
    nodes: usize,
    rows: usize,
}

/// Count allocated row slots, including spare overscan slots outside the current
/// child order. Selection legitimately replaces other panels, such as Inspector.
fn hierarchy_pool(shell: &Shell) -> HierarchyPool {
    let root = shell.ui.find("hierarchy-rows").expect("hierarchy");
    let belongs = |mut index: usize| {
        for _ in 0..shell.ui.nodes.len() {
            if index == root {
                return true;
            }
            let Some(parent) = shell.ui.nodes[index].element.parent else {
                return false;
            };
            index = parent;
        }
        panic!("retained hierarchy node contains a parent cycle");
    };
    let retained: Vec<_> = shell
        .ui
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.element.id == "hierarchy-rows" || node.element.id.starts_with("hierarchy-rows/")
        })
        .collect();
    assert!(
        retained.iter().all(|(index, _)| belongs(*index)),
        "a retained hierarchy slot escaped its owning scroll view"
    );
    let rows: Vec<_> = retained
        .iter()
        .filter(|(_, node)| node.element.tag == "button")
        .map(|(_, node)| node)
        .collect();
    let row_height = rows
        .iter()
        .map(|node| node.rect.h)
        .filter(|height| height.is_finite() && *height > 0.)
        .fold(f32::INFINITY, f32::min);
    assert!(!rows.is_empty() && row_height.is_finite());
    let capacity = (shell.ui.nodes[root].rect.h / row_height).ceil() as usize + 2;
    assert!(
        rows.len() <= capacity,
        "retained hierarchy rows exceed the viewport and two overscan slots: {} > {capacity}",
        rows.len()
    );
    HierarchyPool {
        nodes: retained.len(),
        rows: rows.len(),
    }
}

#[test]
fn hierarchy_thumb_drag_home_end_and_arrows_keep_the_pool_bounded() -> Result<()> {
    let project = Project::new()?;
    let mut editor = Editor::open_with(&project.0, Some(project.0.join(".ara/test-user.json")))?;
    // Measure settled retained work without optional colour-transition frames.
    editor.action(
        "editor.settings.set",
        json!({"key":"appearance.reduceMotion","value":true}),
    )?;
    let mut shell = Shell::new(&mut editor, [1440., 900.])?;
    let count = shell.ui.nodes.len();
    let pool = hierarchy_pool(&shell);
    let selection = editor.state.selection.clone();
    let revision = editor.host.store().revision();
    let focus = shell.input.focused();
    let i = shell.ui.find("hierarchy-rows").expect("hierarchy");
    let bar = shell.ui.scrollbars(i)?[0];
    let x = bar.thumb.x + bar.thumb.w * 0.5;
    shell.event(InputEvent::PointerDown([x, bar.thumb.y + 2.]), &mut Clip)?;
    assert!(shell.input.scrolling(), "the scrollbar must own the drag");
    shell.event(
        InputEvent::PointerMove([x, bar.track.y + bar.track.h]),
        &mut Clip,
    )?;
    shell.event(
        InputEvent::PointerUp([x, bar.track.y + bar.track.h]),
        &mut Clip,
    )?;
    // Flush the same widget actions as the production event loop before checking
    // selection: a delayed row activation must not hide behind the thumb frame.
    shell.dispatch(&mut editor)?;
    shell.frame()?;
    assert_eq!(
        editor.state.selection, selection,
        "thumb drag selected an entity"
    );
    assert_eq!(shell.input.focused(), focus, "thumb drag stole row focus");
    assert!(!shell.input.scrolling() && shell.input.captured().is_none());
    assert_eq!(shell.ui.nodes.len(), count);
    assert_eq!(hierarchy_pool(&shell), pool);
    assert!(shell.hierarchy_offset > 10_000.);
    let last = format!(
        "hierarchy-rows/key-{}",
        ara_editor::world::render_id("e_00000511")
    );
    shell.input.focus(&mut shell.ui, stable_id(&last))?;
    assert!(shell.key(&mut editor, 74, true, &mut Clip)?);
    shell.frame()?;
    assert_eq!(shell.hierarchy_offset, 0.);
    assert_eq!(editor.state.selection, ["e_00000000"]);
    assert!(
        !shell
            .ui
            .find("inspector-empty")
            .is_some_and(|index| shell.ui.shown(index))
    );
    assert!(
        shell
            .ui
            .find("inspector-components")
            .is_some_and(|index| shell.ui.shown(index))
    );
    assert_eq!(hierarchy_pool(&shell), pool);
    let selected_count = shell.ui.nodes.len();
    eprintln!(
        "Hierarchy thumb/selection: initial editor nodes={count}, selected editor nodes={selected_count}, retained pool={pool:?}"
    );
    assert!(shell.key(&mut editor, 77, true, &mut Clip)?);
    shell.frame()?;
    assert_eq!(shell.input.focused(), Some(stable_id(&last)));
    assert_eq!(editor.state.selection, ["e_00000511"]);
    assert_eq!(hierarchy_pool(&shell), pool);
    assert_eq!(shell.ui.nodes.len(), selected_count);
    assert!(shell.key(&mut editor, 82, true, &mut Clip)?);
    shell.frame()?;
    let previous = format!(
        "hierarchy-rows/key-{}",
        ara_editor::world::render_id("e_00000510")
    );
    assert_eq!(shell.input.focused(), Some(stable_id(&previous)));
    assert_eq!(editor.state.selection, ["e_00000510"]);
    assert_eq!(hierarchy_pool(&shell), pool);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(shell.frame()?.paint_nodes, 0);
    Ok(())
}
