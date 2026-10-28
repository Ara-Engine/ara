use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("probe-ui")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Probe UI","main":"levels/main.scene"}))).unwrap();
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_probeui","records":{}}),
            ),
        )
        .unwrap();
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(super) struct Fixture {
    pub editor: Editor,
    pub shell: Shell,
    _project: Project,
}
impl Fixture {
    pub fn new() -> Self {
        let project = Project::new();
        let profile = project.0.join(".ara/test-profile");
        std::fs::create_dir_all(&profile).unwrap();
        let mut editor =
            Editor::open_with(&project.0, Some(profile.join("editor-settings.json"))).unwrap();
        editor
            .action(
                "editor.layout.move",
                json!({"panel":"lighting","slot":"view"}),
            )
            .unwrap();
        let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
        shell.offscreen = true;
        Self {
            editor,
            shell,
            _project: project,
        }
    }
    pub fn create(&mut self) -> String {
        self.shell
            .create_lighting(&mut self.editor, crate::shell::lighting::Create::Probe)
            .unwrap();
        self.editor.state.selection[0].clone()
    }
    pub fn probe(&self, entity: &str) -> ara_components::render::ReflectionProbe {
        let rows = crate::world::resolve(self.editor.host.store(), &self.editor.state.doc).unwrap();
        serde::Deserialize::deserialize(&rows[entity].record["ReflectionProbe"]).unwrap()
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
pub(super) fn click(shell: &mut Shell, editor: &mut Editor, id: &str) {
    let node = &shell.ui.nodes[shell.shown_node(id).unwrap()];
    let rect = node.rect.intersection(node.clip);
    assert!(
        rect.w > 0. && rect.h > 0.,
        "{id}: {:?}/{:?}",
        node.rect,
        node.clip
    );
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    assert_eq!(
        shell
            .ui
            .hit_test(point[0], point[1])
            .map(|index| shell.ui.nodes[index].element.id.as_str()),
        Some(id)
    );
    shell
        .event(InputEvent::PointerDown(point), &mut Clip)
        .unwrap();
    shell
        .event(InputEvent::PointerUp(point), &mut Clip)
        .unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
}
pub(super) fn receipt(doc: &str, entity: &str) -> Value {
    json!({"doc":doc,"entity":entity,"generation":2,"sourceRevision":7,"sourceHash":"source-proof","captureHash":"capture-proof",
        "resolution":256,"faces":6,"cachePath":".ara/cache/probes/proof","published":true})
}
pub(super) fn snapshot(doc: &str, entity: &str, version: u64, capable: bool) -> ProbeSnapshot {
    serde_json::from_value(json!({"version":version,"sceneRevision":7,
        "capabilities":{"reflectionProbes":capable,"giDiffuse":false,"overdraw":false,
            "reasons":if capable {json!({})} else {json!({"reflectionProbes":"This CPU fixture has no capture renderer"})}},
        "probes":[{"doc":doc,"entity":entity,"enabled":true,"generation":2,"state":"progress","stage":"faces","progress":0.5,
            "completedWork":3,"totalWork":6,"cancellable":true,"sceneRevision":7,"sourceHash":"source-proof","stale":true,"retainedPrevious":true,
            "lastReceipt":receipt(doc,entity),"diagnostics":[]}]})).unwrap()
}
