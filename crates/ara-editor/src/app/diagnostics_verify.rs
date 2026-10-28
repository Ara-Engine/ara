//! J7 Console/Problems acceptance through the production SDL event route.
mod fixture;
mod proof;
use super::input_verify::native;
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use ara_ui_text::Clipboard;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const FINISH: u32 = 6;
#[derive(Clone, Copy)]
enum Act {
    ClearInitial,
    Seed,
    Filter,
    Collapse,
    ErrorsOff,
    WarningsOff,
    InfoOff,
    Restore,
    Search,
    SearchLocations,
    Source,
    SearchAll,
    ClearPlayOn,
    Context,
    ClearPlayOff,
    Clear,
    NewMessage,
    Problems,
    Why,
    ProblemSource,
    RepairSearch,
    RepairReview,
    RepairCopy,
    RepairClose,
    Cleanup,
}
const ACTS: &[Act] = &[
    Act::ClearInitial,
    Act::Seed,
    Act::Filter,
    Act::Collapse,
    Act::ErrorsOff,
    Act::WarningsOff,
    Act::InfoOff,
    Act::Restore,
    Act::Search,
    Act::SearchLocations,
    Act::Source,
    Act::SearchAll,
    Act::ClearPlayOn,
    Act::Context,
    Act::ClearPlayOff,
    Act::Clear,
    Act::NewMessage,
    Act::Problems,
    Act::Why,
    Act::ProblemSource,
    Act::RepairSearch,
    Act::RepairReview,
    Act::RepairCopy,
    Act::RepairClose,
    Act::Cleanup,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::ClearInitial => "console-clear-baseline",
            Self::Seed => "diagnostic-bus-fixture",
            Self::Filter => "console-native-search",
            Self::Collapse => "collapse-identity-and-repeat-count",
            Self::ErrorsOff => "severity-errors-off",
            Self::WarningsOff => "severity-warnings-off",
            Self::InfoOff => "severity-info-off",
            Self::Restore => "severity-restoration",
            Self::Search => "message-search",
            Self::SearchLocations => "same-message-distinct-locations",
            Self::Source => "console-source-line-column",
            Self::SearchAll => "restore-all-fixtures",
            Self::ClearPlayOn => "clear-on-play-toolbar",
            Self::Context => "console-keyboard-context-menu",
            Self::ClearPlayOff => "clear-on-play-context-toggle",
            Self::Clear => "clear-hides-existing-keeps-bus",
            Self::NewMessage => "new-message-after-clear",
            Self::Problems => "problems-same-code-locations",
            Self::Why => "problems-catalog-why",
            Self::ProblemSource => "problems-source-line-column",
            Self::RepairSearch => "repair-fix-reachable",
            Self::RepairReview => "repair-review-close-prerequisite",
            Self::RepairCopy => "repair-exact-command-copy",
            Self::RepairClose => "repair-dismiss-restores-focus",
            Self::Cleanup => "restore-editor-layout",
        }
    }
}

struct MemoryClipboard(String);
impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

pub(super) struct DiagnosticsCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    revision: u64,
    clipboard: MemoryClipboard,
    records: Vec<Value>,
    results: Vec<Value>,
    before: Value,
    initial_panel: String,
    initial_maximized: Option<String>,
    frames: Vec<f64>,
    native_events: usize,
}
impl DiagnosticsCheck {
    pub fn new(output: &Path) -> Self {
        Self {
            output: output.into(),
            index: 0,
            age: 0,
            revision: 0,
            clipboard: MemoryClipboard("J7 isolated clipboard before Copy".into()),
            records: Vec::new(),
            results: Vec::new(),
            before: Value::Null,
            initial_panel: String::new(),
            initial_maximized: None,
            frames: Vec::new(),
            native_events: 0,
        }
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|r| r["ok"] != true)
            .filter_map(|r| r["journey"].as_str().map(str::to_owned))
            .collect()
    }
    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }

    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        let count = events.len();
        let result = self.inject_step(act, editor, shell, window, scale, events);
        self.native_events += events.len() - count;
        if let Err(error) = &result {
            self.results.push(json!({"journey":act.label(),"ok":false,"injectionError":error.to_string(),"frame":self.age,"state":proof::snapshot(shell)}));
            self.write()?;
        }
        result
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "native journey borrows loop owners independently"
    )]
    fn inject_step(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.index == 0 && self.age == 0 {
            if !shell.offscreen {
                return Err("J7 requires a hidden offscreen editor".into());
            }
            if shell.toolkit_verify.is_some() || shell.modal_active() {
                return Err("J7 requires the previous modal journey to finish first".into());
            }
            shell.leave();
            self.initial_panel = editor.state.focus.clone();
            self.initial_maximized = editor.state.maximized.clone();
            self.revision = editor.host.store().revision();
            show(editor, shell, "console")?;
        }
        if self.age == 0 {
            if matches!(act, Act::Problems) {
                show(editor, shell, "problems")?;
            }
            self.records = fixture::records(editor)?;
            self.before = proof::snapshot(shell);
            return Ok(());
        }
        let source = || fixture::seq(&self.records, fixture::SAME, fixture::FILE_A, 12);
        match (act, self.age) {
            (Act::ClearInitial, 1) => click(shell, window, scale, "console-clear", events)?,
            (Act::ClearInitial, 2) => {
                set_toggle(shell, window, scale, "console-errors", true, events)?
            }
            (Act::ClearInitial, 3) => {
                set_toggle(shell, window, scale, "console-warnings", true, events)?
            }
            (Act::ClearInitial, 4) => {
                set_toggle(shell, window, scale, "console-info", true, events)?
            }
            (Act::ClearInitial, 5) => {
                set_toggle(shell, window, scale, "console-collapse", false, events)?;
                set_toggle(shell, window, scale, "console-clear-play", false, events)?;
            }
            (Act::Seed, 1) => {
                fixture::seed(editor)?;
                shell.rebuild(editor)?;
            }
            (Act::Filter, 1)
            | (Act::Search, 1)
            | (Act::SearchLocations, 1)
            | (Act::SearchAll, 1) => click(shell, window, scale, "search-console", events)?,
            (Act::Filter, 2) | (Act::SearchAll, 2) => replace(window, "J7 ", events)?,
            (Act::Search, 2) => replace(window, "J7 warning only", events)?,
            (Act::SearchLocations, 2) => replace(window, fixture::SAME, events)?,
            (Act::Collapse, 1) => {
                set_toggle(shell, window, scale, "console-collapse", true, events)?
            }
            (Act::ErrorsOff, 1) => {
                set_toggle(shell, window, scale, "console-errors", false, events)?
            }
            (Act::WarningsOff, 1) => {
                set_toggle(shell, window, scale, "console-warnings", false, events)?
            }
            (Act::InfoOff, 1) => set_toggle(shell, window, scale, "console-info", false, events)?,
            (Act::Restore, 1) => set_toggle(shell, window, scale, "console-errors", true, events)?,
            (Act::Restore, 2) => {
                set_toggle(shell, window, scale, "console-warnings", true, events)?
            }
            (Act::Restore, 3) => set_toggle(shell, window, scale, "console-info", true, events)?,
            (Act::Source, 1) => click(
                shell,
                window,
                scale,
                &format!("log-{}-source", source()?),
                events,
            )?,
            (Act::ClearPlayOn, 1) => {
                set_toggle(shell, window, scale, "console-clear-play", true, events)?
            }
            (Act::Context, 1) => click(
                shell,
                window,
                scale,
                &format!("log-{}-source", source()?),
                events,
            )?,
            (Act::Context, 2) => native::tap(window, 101, events)?,
            (Act::ClearPlayOff, 1) => {
                let id = proof::menu_command(shell, "console.clear_on_play")
                    .ok_or("J7 Console context menu lacks Clear on Play")?;
                click(shell, window, scale, &id, events)?;
            }
            (Act::Clear, 1) => click(shell, window, scale, "console-clear", events)?,
            (Act::NewMessage, 1) => {
                fixture::after_clear(editor)?;
                shell.rebuild(editor)?;
            }
            (Act::Problems, 1) | (Act::RepairSearch, 1) => {
                click(shell, window, scale, "search-problems", events)?
            }
            (Act::Problems, 2) => replace(window, fixture::SAME, events)?,
            (Act::Why, 1) => click(
                shell,
                window,
                scale,
                &format!("problem-{}-why", source()?),
                events,
            )?,
            (Act::ProblemSource, 1) => click(
                shell,
                window,
                scale,
                &format!("problem-{}-go", source()?),
                events,
            )?,
            (Act::RepairSearch, 2) => replace(window, fixture::REPAIR, events)?,
            (Act::RepairReview, 1) => {
                let seq = fixture::seq(&self.records, fixture::REPAIR, fixture::FILE_B, 20)?;
                click(
                    shell,
                    window,
                    scale,
                    &format!("problem-{seq}-fix-0"),
                    events,
                )?;
            }
            (Act::RepairCopy, 1) => click(shell, window, scale, "dialog-copy", events)?,
            (Act::RepairClose, 1) => native::tap(window, 41, events)?,
            (Act::Cleanup, 1) => {
                editor.action(
                    "editor.view",
                    json!({"maximized":self.initial_maximized.as_deref().unwrap_or("")}),
                )?;
                editor.action("editor.focus", json!({"panel":self.initial_panel}))?;
                shell.rebuild(editor)?;
            }
            _ => {}
        }
        if self.age <= 2 {
            self.records = fixture::records(editor)?;
        }
        Ok(())
    }

    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        frame_ms: f64,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        if frame_ms.is_finite() {
            self.frames.push(frame_ms);
        }
        if self.age == 0 {
            self.capture(shell, gpu, act, "before")?;
        }
        if self.age < FINISH {
            self.age += 1;
            return Ok(());
        }
        let mut checks = proof::checks(
            act,
            shell,
            &self.records,
            &self.clipboard.0,
            editor.host.root(),
        );
        checks.push(json!({"check":"diagnostic interactions do not mutate authoring content","ok":editor.host.store().revision()==self.revision}));
        if matches!(act, Act::Cleanup) {
            checks.push(json!({"check":"original panel and maximized state restored","ok":editor.state.focus==self.initial_panel&&editor.state.maximized==self.initial_maximized}));
        }
        self.capture(shell, gpu, act, "after")?;
        self.frames.sort_by(f64::total_cmp);
        let ok = checks.iter().all(|check| check["ok"] == true);
        self.results.push(json!({"journey":act.label(),"ok":ok,"checks":checks,"before":self.before,"after":proof::snapshot(shell),"bus":self.records,"frameP50Ms":self.frames.get(self.frames.len()/2),"frameP95Ms":self.frames.get(self.frames.len()*95/100),"frameMaxMs":self.frames.last()}));
        self.index += 1;
        self.age = 0;
        self.frames.clear();
        self.write()
    }
    fn capture(
        &self,
        shell: &Shell,
        gpu: &mut NativeUi,
        act: Act,
        stage: &str,
    ) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        let stem = format!("diagnostics-{:02}-{}-{stage}", self.index + 1, act.label());
        gpu.select_target(0);
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("diagnostics.json"),
            ara_json::to_canonical_string(
                &json!({"route":"raw SDL -> Window::poll_tagged -> production editor Console/Problems controls","fixture":"typed diag.report; source locations deliberately include missing C# files","clipboard":"isolated memory; OS clipboard service unverified","sourceEditor":"offscreen records the exact file/line/column; no external IDE launch","clearOnPlay":"toolbar and context toggle verified here; player-start clearing is outside this leaf","nativeEvents":self.native_events,"completed":self.finished(),"passed":self.results.iter().filter(|r|r["ok"]==true).count(),"failed":self.failures().len(),"journeys":self.results}),
            ),
        )?;
        Ok(())
    }
}

fn show(editor: &mut Editor, shell: &mut Shell, panel: &str) -> ara_ui::Result<()> {
    editor.action("editor.layout.show", json!({"panel":panel}))?;
    editor.action("editor.focus", json!({"panel":panel}))?;
    editor.action("editor.view", json!({"maximized":panel}))?;
    shell.rebuild(editor)?;
    Ok(())
}
fn click(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let i = shell
        .shown_node(id)
        .ok_or_else(|| format!("J7 control is not shown: {id}"))?;
    let rect = shell.ui.nodes[i].rect.intersection(shell.ui.nodes[i].clip);
    if rect.w < 1. || rect.h < 1. {
        return Err(format!("J7 control is clipped and unreachable: {id}").into());
    }
    native::motion(
        window,
        [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
        scale,
        events,
    )?;
    native::button(window, true, events)?;
    native::button(window, false, events)
}
fn set_toggle(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    on: bool,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    if proof::checked(shell, id) != on {
        click(shell, window, scale, id, events)?;
    }
    Ok(())
}
fn replace(window: &Window, text: &str, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    native::shortcut(window, 4, events)?;
    native::text(window, text, events)
}
