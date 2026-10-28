use super::*;
use crate::shell::assets::scan_fixture::controlled;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn storage(path: &str) -> String {
    if path.is_empty() {
        ara_doc::AUTHORING_ROOT.into()
    } else {
        format!("{}/{path}", ara_doc::AUTHORING_ROOT)
    }
}

pub(super) struct Project(pub PathBuf);
impl Project {
    pub fn new() -> ara_ui::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())?
            .join("asset-bookmarks")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(storage("levels")))?;
        let scene_storage = storage("levels/main.scene");
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Asset bookmarks","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"levels/main.scene"}),
            ),
            (
                scene_storage.as_str(),
                json!({"$schema":"ara://schema/scene/1","id":"sc_bookmarks","records":{"e_bookmarks":{"name":"Context","Transform":{}}}}),
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
            && self.0.starts_with(parent.join("asset-bookmarks"))
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
pub(super) struct Fixture {
    pub editor: Editor,
    pub shell: Shell,
    _project: Project,
}
impl Fixture {
    pub fn new(size: [f32; 2]) -> ara_ui::Result<Self> {
        let project = Project::new()?;
        let mut editor =
            Editor::open_with(&project.0, Some(project.0.join(".ara/user-test.json")))?;
        editor.action("editor.preferences", json!({"reduceMotion":true}))?;
        editor.action("editor.select", json!({"entities":["e_bookmarks"]}))?;
        editor.action(
            "editor.layout.move",
            json!({"panel":"assets","slot":"view"}),
        )?;
        editor.action("editor.view", json!({"maximized":"assets"}))?;
        editor.action(
            "editor.workspace",
            json!({"folder":storage("Art"),"grid":false}),
        )?;
        let mut shell = Shell::new(&mut editor, size)?;
        shell.offscreen = true;
        let (browser, mut worker) = controlled(ara_doc::AUTHORING_ROOT);
        shell.browser = browser;
        let mut items = vec![assets::Asset::folder(
            ara_doc::AUTHORING_ROOT.into(),
            Some(String::new()),
        )];
        for folder in [storage("Art"), storage("Art/Nested"), storage("Artist")] {
            items.push(assets::Asset::folder(
                folder.clone(),
                folder
                    .strip_prefix(ara_doc::AUTHORING_ROOT)
                    .and_then(|tail| tail.strip_prefix('/'))
                    .map(str::to_owned),
            ));
        }
        for path in [
            storage("Art/a.prefab"),
            storage("Art/Nested/b.prefab"),
            storage("Artist/outside.prefab"),
        ] {
            let mut asset = assets::Asset::unindexed(path.clone(), "prefab".into());
            asset.authored_path = path
                .strip_prefix(ara_doc::AUTHORING_ROOT)
                .and_then(|tail| tail.strip_prefix('/'))
                .map(str::to_owned);
            asset.document_kind = Some("prefab".into());
            items.push(asset);
        }
        let scan = worker.start();
        worker.publish(scan, items)?;
        shell.browser.poll();
        shell.rebuild(&mut editor)?;
        shell.frame()?;
        Ok(Self {
            editor,
            shell,
            _project: project,
        })
    }
}
pub(super) struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
pub(super) fn navigation_revision(editor: &mut Editor) -> ara_ui::Result<usize> {
    Ok(editor.action("editor.history", json!({}))?["items"]
        .as_array()
        .ok_or("Navigation history missing")?
        .len())
}
