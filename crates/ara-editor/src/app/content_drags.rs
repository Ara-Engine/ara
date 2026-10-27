//! `--verify-behaviours`: content gestures through the production native event router.
//! Every capture is read from a hidden window's completed native target.
use super::*;
use ara_ui::paint::Box2;
use std::path::Path;
mod fixture;
mod input;
mod verify;
pub(super) use fixture::fixture;

const IDS: [&str; 5] = [
    "e_draga01",
    "e_dragb01",
    "e_dragc01",
    "e_dragd01",
    "e_drage01",
];
const CAMERA: &str = "e_dragcam1";
const SETTLE: u32 = 6;
const BEGIN: u32 = SETTLE + 1;
const PREVIEW: u32 = BEGIN + 4;
const RELEASE: u32 = BEGIN + 7;
const FINISH: u32 = RELEASE + SETTLE;
const PATIENCE: u32 = 144 * 120;

#[derive(Clone, Copy, Debug)]
enum Act {
    MultiReorder,
    Reparent,
    Cycle,
    Cancel,
    AssetReference,
    InvalidAsset,
    AssetSurface,
    MaterialSurface,
    EntityReference,
    Component,
    FloatingReference,
    OsImport,
}
const ACTS: &[Act] = &[
    Act::MultiReorder,
    Act::Reparent,
    Act::Cycle,
    Act::Cancel,
    Act::AssetReference,
    Act::InvalidAsset,
    Act::AssetSurface,
    Act::MaterialSurface,
    Act::EntityReference,
    Act::Component,
    Act::FloatingReference,
    Act::OsImport,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::MultiReorder => "hierarchy-multi-reorder",
            Self::Reparent => "hierarchy-reparent",
            Self::Cycle => "hierarchy-invalid-cycle",
            Self::Cancel => "drag-escape",
            Self::AssetReference => "asset-reference",
            Self::InvalidAsset => "asset-invalid-reference",
            Self::AssetSurface => "asset-surface-placement",
            Self::MaterialSurface => "material-submesh-drop",
            Self::EntityReference => "entity-reference",
            Self::Component => "component-reorder",
            Self::FloatingReference => "floating-reference",
            Self::OsImport => "os-file-import",
        }
    }
}
#[derive(Clone, Copy)]
struct Point {
    local: [f32; 2],
    desktop: [f32; 2],
    scale: f32,
    sdl: u32,
}

pub(super) struct ContentDrags {
    output: PathBuf,
    index: usize,
    age: u32,
    waiting: bool,
    waited: u32,
    failed_frame: bool,
    source: Option<Point>,
    before_revision: u64,
    before_entities: Vec<String>,
    results: Vec<Value>,
    trace: Vec<Value>,
}
impl ContentDrags {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Behaviour verification requires its isolated fixture project".into());
        }
        let mut commands: Vec<_> = IDS.iter().enumerate().map(|(n, id)| json!({"cmd":"entity.create","args":{"id":id,"name":format!("Drag object {}",n+1),"components":{"Transform":{"position":[n as f32 * 2. - 4.,0.5,0.]},"MeshRenderer":{}}}})).collect();
        commands.push(json!({"cmd":"entity.create","args":{"id":CAMERA,"name":"Follow camera","components":{"Transform":{"position":[0.,3.,-6.]},"Camera":{},"FollowCamera":{}}}}));
        commands.push(json!({"cmd":"entity.create","args":{"id":"e_dragfloor","name":"Drop surface","components":{"Transform":{},"MeshRenderer":{"shape":"plane","size":[30.,1.,30.]}}}}));
        editor.apply(commands, "Create isolated behaviour fixtures", None)?;
        editor.action(
            "editor.resource.create",
            json!({"path":"assets/coating.graph","domain":"material"}),
        )?;
        editor.action("editor.preferences", json!({"reduceMotion":true}))?;
        std::fs::create_dir_all(output)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            waited: 0,
            failed_frame: false,
            source: None,
            before_revision: 0,
            before_entities: Vec::new(),
            results: Vec::new(),
            trace: Vec::new(),
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["behaviour"].as_str().map(str::to_owned))
            .collect()
    }
    /// Queue native input before the loop dispatches its event batch.
    #[allow(clippy::too_many_arguments)]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        floats: &windows::Floats,
        scale: f32,
        scene: &viewport::SceneView,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        if let Err(error) = self.input(act, editor, shell, window, floats, scale, scene, events) {
            shell.leave();
            shell.desktop_pointer = None;
            events.retain(|(_, event)| !matches!(event, Event::MouseButton { .. }));
            events.push((
                0,
                Event::Key {
                    scancode: 41,
                    pressed: true,
                    repeat: false,
                },
            ));
            self.results.push(json!({"behaviour":act.label(),"ok":false,"error":error.to_string(),"trace":self.trace}));
            self.advance();
            self.failed_frame = true;
            self.write()?;
        }
        Ok(())
    }
    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
        self.waited = 0;
        self.source = None;
        self.trace.clear();
    }
    /// Capture an already-composited frame and verify the mutation after the gesture settles.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.failed_frame) || self.finished() {
            return Ok(());
        }
        if self.waiting {
            self.waited += 1;
            return Ok(());
        }
        let act = ACTS[self.index];
        if [SETTLE, PREVIEW, FINISH].contains(&self.age) {
            let stage = if self.age == SETTLE {
                "before"
            } else if self.age == PREVIEW {
                "preview"
            } else {
                "after"
            };
            let prefix = format!("{:02}-{}-{stage}", self.index + 1, act.label());
            gpu.select_target(0);
            gpu.pixels()?
                .save(self.output.join(format!("{prefix}.png")))?;
            std::fs::write(
                self.output.join(format!("{prefix}.layout.json")),
                ara_json::to_canonical_string(&shell.layout_report()),
            )?;
            for float in &floats.windows {
                gpu.select_target(float.key);
                gpu.pixels()?
                    .save(self.output.join(format!("{prefix}-{}.png", float.id)))?;
            }
            gpu.select_target(0);
            self.trace.push(json!({"stage":stage,"pointer":shell.pointer,"dragging":shell.content_dragging(),"drop":shell.content_drag_report(),"layout":shell.layout_report()}));
        }
        if self.age >= FINISH {
            let (ok, observed) = self
                .observe(act, editor, shell)
                .unwrap_or_else(|error| (false, json!({"error":error.to_string()})));
            self.results.push(json!({"behaviour":act.label(),"ok":ok,"beforeRevision":self.before_revision,"afterRevision":editor.host.store().revision(),"observed":observed,"trace":self.trace,"capturePrefix":format!("{:02}-{}", self.index+1,act.label())}));
            shell.desktop_pointer = None;
            self.advance();
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("behaviours.json"),
            ara_json::to_canonical_string(
                &json!({"behaviours":self.results,"passed":self.results.iter().filter(|result| result["ok"]==true).count(),"failed":self.failures().len(),"completed":self.finished()}),
            ),
        )?;
        let mut html = String::from(
            "<!doctype html><meta charset='utf-8'><title>Ara editor behaviour evidence</title><style>body{font:14px system-ui;background:#17181b;color:#eee;margin:24px}section{margin:24px 0}div{display:flex;gap:12px}figure{margin:0;flex:1;min-width:0}img{width:100%}figcaption{margin-top:6px}</style><h1>Ara preview 4 editor behaviour evidence</h1><p>Hidden native windows; production event router. Exact outcomes and layout bounds are in <a href='behaviours.json'>behaviours.json</a>.</p>",
        );
        for (index, act) in ACTS.iter().enumerate().take(self.index) {
            html.push_str(&format!(
                "<section><h2>{}. {}</h2><div>",
                index + 1,
                act.label()
            ));
            for stage in ["before", "preview", "after"] {
                let file = format!("{:02}-{}-{stage}.png", index + 1, act.label());
                html.push_str(&format!("<figure><a href='{file}'><img src='{file}'></a><figcaption>{stage}</figcaption></figure>"));
            }
            html.push_str("</div></section>");
        }
        std::fs::write(self.output.join("index.html"), html)?;
        Ok(())
    }
}
