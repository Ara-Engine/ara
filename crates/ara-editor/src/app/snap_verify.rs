//! J2 mesh snap proof: native pointer/key events, transient previews and one typed undo.
use super::*;
use std::path::Path;
mod fixture;
mod input;
mod proof;

const SOURCE: &str = "e_snapmesh01";
const CHILD: &str = "e_snapchild01";
const TARGET: &str = "e_snapramp01";
const EMPTY: &str = "e_snapempty01";
const BEFORE: u32 = 6;
const PREVIEW: u32 = 12;
const AFTER: u32 = 18;
const FINISH: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Surface,
    Vertex,
    Escape,
    Invalid,
    Drop,
    DropInvalid,
    Ortho,
    Floating,
    DeletedTarget,
}
const ACTS: &[Act] = &[
    Act::Surface,
    Act::Vertex,
    Act::Escape,
    Act::Invalid,
    Act::Drop,
    Act::DropInvalid,
    Act::Ortho,
    Act::Floating,
    Act::DeletedTarget,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Surface => "tilted-surface-offset-grid-bypass",
            Self::Vertex => "source-target-vertex",
            Self::Escape => "snap-escape-no-ops",
            Self::Invalid => "invalid-snap-release-no-ops",
            Self::Drop => "drop-to-actual-surface",
            Self::DropInvalid => "drop-selection-atomic-rejection",
            Self::Ortho => "orthographic-surface-contact",
            Self::Floating => "floating-vertex-contact",
            Self::DeletedTarget => "deleted-target-cancels-snap",
        }
    }
    fn vertex(self) -> bool {
        matches!(self, Self::Vertex | Self::Floating)
    }
    fn drop(self) -> bool {
        matches!(self, Self::Drop | Self::DropInvalid)
    }
    fn commits(self) -> bool {
        matches!(
            self,
            Self::Surface | Self::Vertex | Self::Drop | Self::Ortho | Self::Floating
        )
    }
}

pub(super) struct SnapCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    waiting: bool,
    waited: u32,
    from: [f32; 2],
    to: [f32; 2],
    delta: Vec3,
    source: Option<Vec3>,
    target: Option<Vec3>,
    normal: Option<Vec3>,
    vertices: Vec<Vec3>,
    revision: u64,
    before: Value,
    committed: Option<Value>,
    trace: Vec<Value>,
    results: Vec<Value>,
}
impl SnapCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Snap verification requires the isolated behaviour fixture".into());
        }
        fixture::create(editor)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            waited: 0,
            from: [0.; 2],
            to: [0.; 2],
            delta: Vec3::ZERO,
            source: None,
            target: None,
            normal: None,
            vertices: Vec::new(),
            revision: 0,
            before: Value::Null,
            committed: None,
            trace: Vec::new(),
            results: Vec::new(),
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["journey"].as_str().map(str::to_owned))
            .collect()
    }

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
        if self.age == 0 {
            return self.setup(act, editor, shell);
        }
        if self.age == BEFORE && !self.acquire(act, editor, shell, scene)? {
            self.waiting = true;
            self.waited += 1;
            if self.waited > 144 * 60 {
                return Err(
                    format!("Native snap fixture did not become ready: {}", act.label()).into(),
                );
            }
            return Ok(());
        }
        self.input(act, editor, shell, window, floats, scale, events)
    }

    /// Capture the owning hidden native target after the production frame presents.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        if self.waiting || self.finished() {
            return Ok(());
        }
        let act = ACTS[self.index];
        if [BEFORE, PREVIEW, AFTER, 22, 26].contains(&self.age) {
            let stage = match self.age {
                BEFORE => "before",
                PREVIEW => "preview",
                AFTER => "after",
                22 => "undo",
                _ => "redo",
            };
            self.capture(act, stage, editor, shell, gpu, floats)?;
            if self.age == AFTER {
                self.committed = Some(fixture::records(editor)?);
            }
        }
        if self.age >= FINISH {
            let (ok, detail) = self.result(act, editor)?;
            self.results
                .push(json!({"journey":act.label(),"ok":ok,"detail":detail,"trace":self.trace}));
            self.index += 1;
            self.age = 0;
            self.waited = 0;
            self.trace.clear();
            self.committed = None;
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("snapping.json"),
            ara_json::to_canonical_string(
                &json!({"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len(),"journeys":self.results,"geometry":"presented native mesh triangles and primary-LOD vertices; no ground-plane fallback","input":"SDL events tagged to the Scene host with its own display/UI scale"}),
            ),
        )?;
        Ok(())
    }
}
