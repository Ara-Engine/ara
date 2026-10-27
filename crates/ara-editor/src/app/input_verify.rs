//! J14: SDL → native editor routing → shared retained controls, on a hidden target.
//! The clipboard is memory-only; this journey never reads or replaces the OS clipboard.
mod native;
mod proof;
use crate::{
    Editor,
    gpu::NativeUi,
    shell::{Shell, toolkit_verify::Host},
};
use ara_platform::{Event, Window};
use ara_ui::paint::Box2;
use ara_ui_text::Clipboard;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const UNICODE: &str = "Ara — مرحبا 你好 🙂";
const PREEDIT: &str = "临时🙂";
const FINISH: u32 = 6;

#[derive(Clone, Copy)]
enum Act {
    Unicode,
    Clipboard,
    Grapheme,
    Preedit,
    Cancel,
    FocusLoss,
    SelectEnd,
    SelectSearch,
    SelectAccept,
    Prompt,
    Stack,
    Unstack,
    PromptAccept,
    LongNote,
    NoteClose,
    Cleanup,
}
const ACTS: &[Act] = &[
    Act::Unicode,
    Act::Clipboard,
    Act::Grapheme,
    Act::Preedit,
    Act::Cancel,
    Act::FocusLoss,
    Act::SelectEnd,
    Act::SelectSearch,
    Act::SelectAccept,
    Act::Prompt,
    Act::Stack,
    Act::Unstack,
    Act::PromptAccept,
    Act::LongNote,
    Act::NoteClose,
    Act::Cleanup,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Unicode => "unicode-single-ingress",
            Self::Clipboard => "clipboard-home-end-selection",
            Self::Grapheme => "grapheme-shift-selection",
            Self::Preedit => "ime-preedit",
            Self::Cancel => "ime-escape-cancel",
            Self::FocusLoss => "focus-loss-held-state",
            Self::SelectEnd => "select-100-end-reveal",
            Self::SelectSearch => "select-localized-search",
            Self::SelectAccept => "select-accept-restores-focus",
            Self::Prompt => "prompt-tab-trap",
            Self::Stack => "modal-stack-long-notes",
            Self::Unstack => "modal-escape-one-level",
            Self::PromptAccept => "prompt-enter-result",
            Self::LongNote => "long-note-viewport",
            Self::NoteClose => "note-escape-restores-focus",
            Self::Cleanup => "editor-focus-restoration",
        }
    }
}

#[derive(Default)]
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

pub(super) struct InputCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    revision: u64,
    camera: crate::state::Camera,
    clipboard: MemoryClipboard,
    results: Vec<Value>,
    trace: Vec<Value>,
    before: Value,
    stacked_focus: Option<u64>,
    initial_focus: Option<u64>,
    native_events: usize,
    frame_ms: Vec<f64>,
    intermediate: Vec<Value>,
}

impl InputCheck {
    pub fn new(output: &Path) -> Self {
        Self {
            output: output.into(),
            index: 0,
            age: 0,
            revision: 0,
            camera: Default::default(),
            clipboard: Default::default(),
            results: Vec::new(),
            trace: Vec::new(),
            before: Value::Null,
            stacked_focus: None,
            initial_focus: None,
            native_events: 0,
            frame_ms: Vec::new(),
            intermediate: Vec::new(),
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
    /// Supply this only during the active J14 route; the desktop clipboard is untouched.
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
            self.results.push(json!({"journey":act.label(),"ok":false,"injectionError":error.to_string(),"age":self.age,"state":proof::snapshot(shell)}));
            self.write()?;
        }
        result
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "journey borrows production loop owners independently"
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
            if shell.toolkit_verify.is_some() {
                return Err("J14 toolkit proof was already mounted".into());
            }
            shell.leave();
            self.revision = editor.host.store().revision();
            self.camera = editor.state.camera;
            self.initial_focus = shell.input.focused();
            shell.toolkit_verify = Some(Host::new());
            shell.rebuild(editor)?;
        }
        if self.age == 0 {
            self.before = proof::snapshot(shell);
            if matches!(act, Act::Stack) {
                self.stacked_focus = shell.input.focused();
            }
            return Ok(());
        }
        match (act, self.age) {
            (Act::Unicode, 1) => click(shell, window, scale, "kit-text", events)?,
            (Act::Unicode, 2) => replace(window, UNICODE, events)?,
            (Act::Clipboard, 1) => {
                native::shortcut(window, 4, events)?;
                native::shortcut(window, 6, events)?;
            }
            (Act::Clipboard, 2) => native::tap(window, 74, events)?,
            (Act::Clipboard, 3) => shift(window, &[77], events)?,
            (Act::Clipboard, 4) => native::shortcut(window, 25, events)?,
            (Act::Grapheme, 1) => replace(window, "A🙂B", events)?,
            (Act::Grapheme, 2) => {
                native::tap(window, 74, events)?;
                shift(window, &[79, 79], events)?;
            }
            (Act::Grapheme, 3) => native::tap(window, 76, events)?,
            (Act::Preedit, 1) => replace(window, "Stable", events)?,
            (Act::Preedit, 2) => native::preedit(window, PREEDIT, events)?,
            (Act::Cancel, 1) => native::tap(window, 41, events)?,
            (Act::FocusLoss, 1) => {
                native::motion(window, point(shell, "kit-open-prompt")?, scale, events)?;
                native::key(window, 229, true, events)?;
                native::button(window, true, events)?;
            }
            (Act::FocusLoss, 2) => native::focus_lost(window, events)?,
            (Act::FocusLoss, 3) => {
                native::button(window, false, events)?;
                native::tap(window, 43, events)?;
            }
            (Act::SelectEnd, 1) => click(shell, window, scale, "kit-select", events)?,
            (Act::SelectEnd, 2) => native::tap(window, 77, events)?,
            (Act::SelectSearch, 1) => native::text(window, "Choice 097", events)?,
            (Act::SelectAccept, 1) => native::tap(window, 40, events)?,
            (Act::Prompt, 1) => click(shell, window, scale, "kit-open-prompt", events)?,
            (Act::Prompt, 2) => native::text(window, "Prompt ✓", events)?,
            (Act::Prompt, 3) => {
                for _ in 0..7 {
                    native::tap(window, 43, events)?;
                }
            }
            (Act::Stack, 1) => native::tap(window, 59, events)?,
            (Act::Unstack, 1) => native::tap(window, 41, events)?,
            (Act::PromptAccept, 1) => click(shell, window, scale, "kit-dialog-1-field", events)?,
            (Act::PromptAccept, 2) => replace(window, "Saved prompt ✓", events)?,
            (Act::PromptAccept, 3) => native::tap(window, 40, events)?,
            (Act::LongNote, 1) => click(shell, window, scale, "kit-open-note", events)?,
            (Act::NoteClose, 1) => native::tap(window, 41, events)?,
            (Act::Cleanup, 1) => {
                if let Some(mut host) = shell.toolkit_verify.take() {
                    host.close(&mut shell.ui, &mut shell.input)?;
                }
                shell.rebuild(editor)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Capture only completed native UI targets, after production event routing/presentation.
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
            self.frame_ms.push(frame_ms);
        }
        if self.age == 0 {
            self.capture(shell, gpu, act, "before")?;
        }
        self.intermediate
            .extend(proof::frame_checks(act, self.age, shell));
        self.trace
            .push(json!({"step":act.label(),"frame":self.age,"state":proof::snapshot(shell)}));
        if self.age < FINISH {
            self.age += 1;
            return Ok(());
        }
        let state = proof::snapshot(shell);
        let mut checks = proof::checks(
            act,
            shell,
            &self.clipboard.0,
            self.stacked_focus,
            self.initial_focus,
        );
        checks.append(&mut self.intermediate);
        checks.push(json!({"check":"document revision unchanged","ok":editor.host.store().revision()==self.revision}));
        checks.push(json!({"check":"camera unchanged behind toolkit scopes","ok":editor.state.camera==self.camera}));
        let ok = checks.iter().all(|check| check["ok"] == true);
        self.capture(shell, gpu, act, "after")?;
        self.frame_ms.sort_by(f64::total_cmp);
        self.results.push(json!({"journey":act.label(),"ok":ok,"checks":checks,"before":self.before,"after":state,"frameP50Ms":self.frame_ms.get(self.frame_ms.len()/2),"frameP95Ms":self.frame_ms.get(self.frame_ms.len()*95/100),"frameMaxMs":self.frame_ms.last()}));
        self.index += 1;
        self.age = 0;
        self.frame_ms.clear();
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
        let stem = format!("input-{:02}-{}-{stage}", self.index + 1, act.label());
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
            self.output.join("native-input.json"),
            ara_json::to_canonical_string(
                &json!({"route":"raw SDL -> Window::poll_tagged -> production editor route -> native_input::Router -> shared Select/Dialogs/InputState","clipboard":"isolated in-memory; OS clipboard service unverified by this journey","ime":"injected preedit/commit; OS candidate-window integration unverified","nativeEvents":self.native_events,"completed":self.finished(),"passed":self.results.iter().filter(|r|r["ok"]==true).count(),"failed":self.failures().len(),"journeys":self.results,"frames":self.trace}),
            ),
        )?;
        Ok(())
    }
}

fn point(shell: &Shell, id: &str) -> ara_ui::Result<[f32; 2]> {
    let index = shell
        .shown_node(id)
        .ok_or_else(|| format!("J14 control is not reachable: {id}"))?;
    let Box2 { x, y, w, h } = shell.ui.nodes[index].rect;
    Ok([x + w * 0.5, y + h * 0.5])
}
fn click(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    native::motion(window, point(shell, id)?, scale, events)?;
    native::button(window, true, events)?;
    native::button(window, false, events)
}
fn replace(window: &Window, text: &str, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    native::shortcut(window, 4, events)?;
    native::text(window, text, events)
}
fn shift(window: &Window, keys: &[i32], events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    native::key(window, 225, true, events)?;
    for &key in keys {
        native::tap(window, key, events)?;
    }
    native::key(window, 225, false, events)
}
