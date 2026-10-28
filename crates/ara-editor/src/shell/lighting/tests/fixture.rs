use super::*;
use std::{path::PathBuf,sync::atomic::{AtomicU64,Ordering}};

pub(super) struct Project(pub PathBuf);
impl Project {
    pub fn new()->Self {
        static NEXT:AtomicU64=AtomicU64::new(0);
        let root=std::path::absolute(crate::test_target()).unwrap().join("lighting-authoring")
            .join(format!("{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path,value) in [
            ("ara.project.json",json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Lighting authoring","main":"levels/main.scene"})),
            ("levels/main.scene",json!({"$schema":"ara://schema/scene/1","id":"sc_lightingtests","records":{}})),
        ] {
            let path=if path=="ara.project.json" {root.join(path)} else {root.join(ara_doc::AUTHORING_ROOT).join(path)};
            std::fs::write(path,ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let parent=std::path::absolute(crate::test_target()).unwrap().join("lighting-authoring");
        assert!(self.0.starts_with(parent));
        let _=std::fs::remove_dir_all(&self.0);
    }
}
pub(super) struct Fixture {
    pub editor:Editor,
    pub shell:Shell,
    _project:Project,
}
impl Fixture {
    pub fn new()->Self {
        let project=Project::new();
        let mut editor=Editor::open(&project.0).unwrap();
        editor.action("editor.preferences",json!({"reduceMotion":true})).unwrap();
        let mut shell=Shell::new(&mut editor,[1100.,760.]).unwrap();
        shell.offscreen=true;
        Self{editor,shell,_project:project}
    }
    pub fn create(&mut self,kind:Create)->String {
        self.shell.create_lighting(&mut self.editor,kind).unwrap();
        self.editor.state.selection[0].clone()
    }
    pub fn component(&self,entity:&str,component:&str)->Value {
        crate::world::resolve(self.editor.host.store(),&self.editor.state.doc).unwrap()[entity].record[component].clone()
    }
}
pub(super) struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self)->ara_ui_text::Result<String>{Ok(String::new())}
    fn write(&mut self,_:&str)->ara_ui_text::Result<()>{Ok(())}
}
