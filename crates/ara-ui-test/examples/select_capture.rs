//! Windowless production-GPU Select captures. The required output path owns every artifact.
use ara_editor_gpu::{NativeUi, open_presentation};
use ara_ui::{
    Result, Ui,
    input::{InputEvent, InputState},
    select::{Select, SelectOption},
    source::stable_id,
    theme::{EDITOR_CSS, EditorTheme},
};
use ara_ui_render::{atlas::TextMode, gpu::Renderer};
use ara_ui_text::{Clipboard, EditEvent, TextSystem};
use std::path::Path;

struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}
fn markup(controls: &[Select<usize>]) -> String {
    let mut out = String::from(
        "<ui id='root'><text id='title' text='Shared native Select' style='font-size:20px' /><text id='subtitle' text='Normal, hover, keyboard focus and unavailable states' /><panel id='form' style='width:340px;gap:12px'>",
    );
    for (index, control) in controls.iter().enumerate() {
        let caption = [
            "Engine version",
            "Pointer hover",
            "Keyboard focus",
            "Unavailable version",
        ][index];
        out.push_str(&format!("<panel id='row-{index}' style='gap:4px'><text id='label-{index}' text='{caption}' />{}</panel>",control.control_markup()));
    }
    out.push_str("</panel>");
    for control in controls {
        out.push_str(&control.popover_markup());
    }
    out.push_str("</ui>");
    out
}
fn mount(
    controls: &mut [Select<usize>],
    ui: &mut Ui,
    input: &mut InputState,
    text: &mut TextSystem,
) -> Result<()> {
    ui.reconcile(&markup(controls))?;
    ui.frame(text)?;
    for control in controls {
        control.sync(ui, input)?;
    }
    ui.frame(text)?;
    Ok(())
}
fn route(
    controls: &mut [Select<usize>],
    ui: &mut Ui,
    input: &mut InputState,
    text: &mut TextSystem,
    event: InputEvent,
    clip: &mut Clip,
) -> Result<()> {
    if let Some(event) = controls[0].event(ui, input, event, clip)? {
        input.event(ui, event, clip)?;
    }
    controls[0].dispatch(ui, input)?;
    mount(controls, ui, input, text)
}
#[allow(
    clippy::too_many_arguments,
    reason = "capture owns independent retained and GPU resources"
)]
fn capture(
    output: &Path,
    stem: &str,
    ui: &mut Ui,
    input: &InputState,
    text: &mut TextSystem,
    renderer: &mut Renderer,
    gpu: &mut NativeUi,
) -> Result<()> {
    let work = ui.frame(text)?;
    let idle = ui.frame(text)?;
    renderer.render(gpu, &ui.display, &text.assets, [640, 520], TextMode::Raster)?;
    gpu.pixels()?.save(output.join(format!("{stem}.png")))?;
    let identity = |id: Option<u64>| {
        id.map(|id|serde_json::json!({"stableId":id,"id":ui.by_id(id).map(|i|ui.nodes[i].element.id.as_str())}))
    };
    let search = ui
        .widgets
        .get(&stable_id("engine-search"))
        .and_then(|widget| {
            if let ara_ui::widgets::Widget::Input(input) = widget {
                Some(input.value())
            } else {
                None
            }
        });
    let report = serde_json::json!({"renderer":"NativeUi/windowless","stage":stem,"focus":identity(input.focused()),"capture":identity(input.captured()),"searchValue":search,"frameWork":{"layout":work.layout_passes,"shape":work.shaped,"paint":work.paint_nodes},"idleWork":{"layout":idle.layout_passes,"shape":idle.shaped,"paint":idle.paint_nodes},"ui":ui.inspect()});
    std::fs::write(
        output.join(format!("{stem}.json")),
        ara_json::to_canonical_string(&report),
    )?;
    Ok(())
}
fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or("Pass an explicit isolated output directory")?;
    std::fs::create_dir_all(&output)?;
    let device = open_presentation(&[])?;
    // SAFETY: The presentation device outlives the adapter; this thread owns its queue.
    let mut gpu = unsafe { NativeUi::new(&device)? };
    let mut renderer = Renderer::default();
    let mut text = TextSystem::bundled()?;
    for (theme, name) in [(EditorTheme::Dark, "dark"), (EditorTheme::Light, "light")] {
        let options = || {
            vec![
                SelectOption::new(0, "0.0.1-alpha"),
                SelectOption::new(1, "0.0.2-nightly"),
                SelectOption::new(2, "Preview 4 — édition française / إصدار المعاينة"),
            ]
        };
        let mut disabled = SelectOption::new(0, "No compatible engine installed");
        disabled.disabled = Some("Install a compatible engine first".into());
        let mut controls = vec![
            Select::new("engine", "Engine version", options(), Some(0)),
            Select::new("hover", "Hover version", options(), Some(0)),
            Select::new("focus", "Focused version", options(), Some(2)),
            Select::new(
                "unavailable",
                "Unavailable version",
                vec![disabled],
                Some(0),
            ),
        ];
        let mut ui = Ui::editor(
            &markup(&controls),
            &format!("{EDITOR_CSS}{}", ara_ui::select::CSS),
            [640., 520.],
        )?;
        ui.switch_theme(theme)?;
        ui.frame(&mut text)?;
        let mut input = InputState::default();
        input.push(
            &mut ui,
            stable_id("root"),
            Some(stable_id(controls[2].control_id())),
            false,
        )?;
        mount(&mut controls, &mut ui, &mut input, &mut text)?;
        let hover = ui.nodes[ui
            .find(controls[1].control_id())
            .ok_or("Hover control missing")?]
        .rect;
        let mut clip = Clip(String::new());
        input.event(
            &mut ui,
            InputEvent::PointerMove([hover.x + hover.w * 0.5, hover.y + hover.h * 0.5]),
            &mut clip,
        )?;
        capture(
            &output,
            &format!("{name}-closed"),
            &mut ui,
            &input,
            &mut text,
            &mut renderer,
            &mut gpu,
        )?;
        let rect = ui.nodes[ui
            .find(controls[0].control_id())
            .ok_or("Engine control missing")?]
        .rect;
        let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        for event in [InputEvent::PointerDown(point), InputEvent::PointerUp(point)] {
            route(
                &mut controls,
                &mut ui,
                &mut input,
                &mut text,
                event,
                &mut clip,
            )?;
        }
        capture(
            &output,
            &format!("{name}-open-empty"),
            &mut ui,
            &input,
            &mut text,
            &mut renderer,
            &mut gpu,
        )?;
        route(
            &mut controls,
            &mut ui,
            &mut input,
            &mut text,
            InputEvent::Edit(EditEvent::Insert("no-compatible-match".into())),
            &mut clip,
        )?;
        capture(
            &output,
            &format!("{name}-no-results"),
            &mut ui,
            &input,
            &mut text,
            &mut renderer,
            &mut gpu,
        )?;
    }
    Ok(())
}
