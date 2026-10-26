//! Native Assets journey: right-click, Create, inline rename, real compile and attach.
//! Runs only offscreen with a scratch profile and captures each visible state.
use super::*;
use ara_ui_text::{Clipboard, EditEvent};
use std::path::Path;
#[derive(Default)]
pub(super) struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}
pub(super) struct AssetsVerification {
    step: u32,
    started: Instant,
    output: PathBuf,
    entity: String,
    attach_job: Option<String>,
}
impl AssetsVerification {
    pub(super) fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        std::fs::create_dir_all(output)?;
        editor.action("editor.layout.show", json!({"panel":"assets"}))?;
        editor.action("editor.workspace", json!({"folder":"Assets/Gameplay"}))?;
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?;
        let entity = resolved
            .keys()
            .next()
            .ok_or("Assets route needs an entity")?
            .clone();
        editor.action("editor.select", json!({"entities":[entity]}))?;
        Ok(Self {
            step: 0,
            started: Instant::now(),
            output: output.into(),
            entity,
            attach_job: None,
        })
    }
    pub(super) fn finished(&self) -> bool {
        self.step >= 9
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        scene: &mut viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        if self.started.elapsed() > Duration::from_secs(120) {
            return Err("Assets script route timed out (see Console and jobs)".into());
        }
        let mut clip = Clip::default();
        let capture = verify::Verification::new(&self.output);
        if matches!(self.step, 4 | 5 | 7) {
            let jobs = editor.action("jobs.list", json!({}))?;
            if let Some(failed) = jobs["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|job| job["status"] == "failed")
            {
                return Err(format!(
                    "Script compile failed: {}",
                    ara_json::to_canonical_string(&failed["diagnostics"])
                )
                .into());
            }
        }
        match self.step {
            0 => {
                if !shell.poll_assets_for_test(editor) {
                    return Ok(false);
                }
                capture.screenshot(
                    "01-folders-collapsed",
                    editor,
                    shell,
                    gpu,
                    ui,
                    renderer,
                    scene,
                )?;
            }
            1 => {
                shell.open_assets_context(editor)?;
                shell.click("m0-0", &mut clip)?;
                shell.dispatch(editor)?;
                capture.screenshot("02-create-menu", editor, shell, gpu, ui, renderer, scene)?;
            }
            2 => {
                let id = shell
                    .ui
                    .nodes
                    .iter()
                    .find(|n| {
                        n.element.tag == "button"
                            && n.element
                                .attrs
                                .get("label")
                                .is_some_and(|l| l == "C# Script")
                    })
                    .ok_or("Create menu has no C# Script")?
                    .element
                    .id
                    .clone();
                shell.click(&id, &mut clip)?;
                shell.dispatch(editor)?;
                if !editor
                    .host
                    .root()
                    .join("Assets/Gameplay/NewBehaviour.cs")
                    .exists()
                {
                    return Err("Script creation did not write its selected folder".into());
                }
                capture.screenshot("03-inline-rename", editor, shell, gpu, ui, renderer, scene)?;
            }
            3 => {
                shell.event(
                    InputEvent::Edit(EditEvent::Insert("PlayerController".into())),
                    &mut clip,
                )?;
                shell.dispatch(editor)?;
                shell.key(editor, 40, true, &mut clip)?;
                let source = std::fs::read_to_string(
                    editor
                        .host
                        .root()
                        .join("Assets/Gameplay/PlayerController.cs"),
                )?;
                if !source.contains("class PlayerController : Behaviour") {
                    return Err("Class name did not follow inline file rename".into());
                }
            }
            4 => {
                if editor
                    .host
                    .store()
                    .schema()
                    .get("PlayerController")
                    .is_none()
                {
                    return Ok(false);
                }
                editor.apply(vec![json!({"cmd":"component.add","args":{"entity":self.entity,"component":"PlayerController"}})],"Attach compiled menu-created script",None)?;
                // Exercise the exact MCP command used by Add Component → New Script.
                editor.action("script.create",json!({"path":"Assets/Gameplay/MovementSystem.cs","template":"system","compile":false}))?;
                let result=editor.action("component.new_script",json!({"path":"Assets/Gameplay/AttachedBehaviour.cs","template":"behaviour","attachTo":[self.entity],"doc":editor.state.doc}))?;
                self.attach_job = result["job"].as_str().map(str::to_owned);
            }
            5 => {
                let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
                    .map_err(|d| format!("{d:?}"))?;
                if !resolved
                    .get(&self.entity)
                    .is_some_and(|e| e.record.contains_key("AttachedBehaviour"))
                {
                    return Ok(false);
                }
                let splitter = shell.ui.nodes[shell
                    .ui
                    .find("asset-sidebar-splitter")
                    .ok_or("Assets splitter missing")?]
                .rect;
                let content = shell.ui.nodes[shell
                    .ui
                    .find("assets-content")
                    .ok_or("Assets content missing")?]
                .rect;
                let start = [splitter.x + splitter.w * 0.5, splitter.y + splitter.h * 0.5];
                let end = [content.x + 236., start[1]];
                shell.event(InputEvent::PointerDown(start), &mut clip)?;
                shell.event(InputEvent::PointerMove(end), &mut clip)?;
                shell.event(InputEvent::PointerUp(end), &mut clip)?;
                shell.dispatch(editor)?;
                if (editor.state.asset_sidebar_width - 236.).abs() > 0.1 {
                    return Err("Assets splitter did not resize".into());
                }
                editor.action("editor.select", json!({"entities":[self.entity]}))?;
                shell.rebuild(editor)?;
                capture.screenshot(
                    "04-compiled-attached",
                    editor,
                    shell,
                    gpu,
                    ui,
                    renderer,
                    scene,
                )?;
            }
            6 => {
                let path = "Assets/Gameplay/PlayerController.cs";
                let source = std::fs::read_to_string(editor.host.root().join(path))?;
                let text=source.replace("    /// <inheritdoc/>","    /// <summary>Hit points stored in ECS.</summary>\n    [Property] public partial int Health { get; set; }\n\n    /// <inheritdoc/>");
                editor.action("script.write", json!({"path":path,"text":text}))?;
            }
            7 => {
                if !editor
                    .host
                    .store()
                    .schema()
                    .get("PlayerController")
                    .is_some_and(|ty| {
                        ty.fields()
                            .iter()
                            .any(|field| field.name.eq_ignore_ascii_case("health"))
                    })
                {
                    return Ok(false);
                }
                shell.rebuild(editor)?;
                capture.screenshot("05-hot-compiled", editor, shell, gpu, ui, renderer, scene)?;
            }
            8 => {
                std::fs::write(
                    self.output.join("assets-scripts.json"),
                    ara_json::to_canonical_string(
                        &json!({"compiled":true,"attached":true,"hotCompiled":true,"entity":self.entity,"path":"Assets/Gameplay/PlayerController.cs","createAndAttachJob":self.attach_job,"sidebarWidth":editor.state.asset_sidebar_width}),
                    ),
                )?;
            }
            _ => {}
        }
        self.step += 1;
        Ok(true)
    }
}
