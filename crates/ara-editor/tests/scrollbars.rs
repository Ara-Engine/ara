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
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"name":"Scroll verification","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
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
            root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
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
#[test]
fn hierarchy_thumb_drag_home_end_and_arrows_keep_the_pool_bounded() -> Result<()> {
    let project = Project::new()?;
    let mut editor = Editor::open(&project.0)?;
    let mut shell = Shell::new(&mut editor, [1440., 900.])?;
    let count = shell.ui.nodes.len();
    let i = shell.ui.find("hierarchy-rows").expect("hierarchy");
    let bar = shell.ui.scrollbars(i)?[0];
    let x = bar.thumb.x + bar.thumb.w * 0.5;
    shell.event(InputEvent::PointerDown([x, bar.thumb.y + 2.]), &mut Clip)?;
    shell.event(
        InputEvent::PointerMove([x, bar.track.y + bar.track.h]),
        &mut Clip,
    )?;
    shell.event(
        InputEvent::PointerUp([x, bar.track.y + bar.track.h]),
        &mut Clip,
    )?;
    shell.frame()?;
    assert_eq!(shell.ui.nodes.len(), count);
    assert!(shell.hierarchy_offset > 10_000.);
    let last = format!(
        "hierarchy-rows/key-{}",
        ara_editor::world::render_id("e_00000511")
    );
    shell.input.focus(&mut shell.ui, stable_id(&last))?;
    assert!(shell.key(&mut editor, 74, true, &mut Clip)?);
    shell.frame()?;
    assert_eq!(shell.hierarchy_offset, 0.);
    assert!(shell.key(&mut editor, 77, true, &mut Clip)?);
    shell.frame()?;
    assert_eq!(shell.input.focused(), Some(stable_id(&last)));
    assert_eq!(shell.ui.nodes.len(), count);
    assert!(shell.key(&mut editor, 82, true, &mut Clip)?);
    shell.frame()?;
    let previous = format!(
        "hierarchy-rows/key-{}",
        ara_editor::world::render_id("e_00000510")
    );
    assert_eq!(shell.input.focused(), Some(stable_id(&previous)));
    assert_eq!(shell.frame()?.paint_nodes, 0);
    Ok(())
}
