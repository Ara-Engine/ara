//! Windowless GPU evidence for long profile names in a 74px Select caption area.
//! Pass an isolated output directory; identical calls and bundled fonts serve both revisions.
use ara_editor_gpu::{NativeUi, open_presentation};
use ara_ui::{
    Result, Ui,
    input::{InputEvent, InputState},
    select::{Select, SelectOption},
    source::stable_id,
    theme::{EDITOR_CSS, EditorTheme},
};
use ara_ui_render::{atlas::TextMode, gpu::Renderer};
use ara_ui_text::{Clipboard, TextSystem};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SIZE: [f32; 2] = [160., 450.];
const PIXELS: [u32; 2] = [160, 450];
const CONTROL: &str = "profile-control";
const STYLES: &str = "
ui { padding:12px; gap:0px; }
.kit-select { width:110px; font-size:12px; flex-shrink:0; }
.kit-select-option { font-size:12px; }
";

#[derive(Default)]
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

struct Surface {
    control: Select<usize>,
    ui: Ui,
    input: InputState,
    text: TextSystem,
    clipboard: Clip,
}
impl Surface {
    fn new(theme: EditorTheme) -> Result<Self> {
        let options = (0..12)
            .map(|index| {
                SelectOption::new(
                    index,
                    format!(
                        "Profile {index:02} — localized Windows shipping configuration · إعدادات البناء والتوزيع"
                    ),
                )
            })
            .collect();
        let mut surface = Self {
            control: Select::new("profile", "Build profile", options, Some(0)),
            ui: Ui::editor(
                "<ui id='root' />",
                &format!("{EDITOR_CSS}{}{STYLES}", ara_ui::select::CSS),
                SIZE,
            )?,
            input: InputState::default(),
            text: TextSystem::bundled()?,
            clipboard: Clip::default(),
        };
        surface.ui.switch_theme(theme)?;
        surface.mount()?;
        surface.input.push(
            &mut surface.ui,
            stable_id("root"),
            Some(stable_id(CONTROL)),
            false,
        )?;
        surface.mount()?;
        Ok(surface)
    }

    fn mount(&mut self) -> Result<()> {
        self.ui.reconcile(&format!(
            "<ui id='root'>{}{}</ui>",
            self.control.control_markup(),
            self.control.popover_markup()
        ))?;
        self.ui.frame(&mut self.text)?;
        self.control.sync(&mut self.ui, &mut self.input)?;
        self.ui.frame(&mut self.text)?;
        Ok(())
    }

    fn route(&mut self, event: InputEvent) -> Result<()> {
        if let Some(event) =
            self.control
                .event(&mut self.ui, &mut self.input, event, &mut self.clipboard)?
        {
            self.input.event(&mut self.ui, event, &mut self.clipboard)?;
        }
        self.control.dispatch(&mut self.ui, &mut self.input)?;
        self.mount()
    }

    fn open(&mut self) -> Result<()> {
        let index = self.ui.find(CONTROL).ok_or("Profile control missing")?;
        let rect = self.ui.nodes[index].rect;
        let clip = rect.intersection(self.ui.nodes[index].clip);
        if clip.w <= 0. || clip.h <= 0. {
            return Err("Profile control has no pointer target".into());
        }
        let point = [clip.x + clip.w * 0.5, clip.y + clip.h * 0.5];
        self.route(InputEvent::PointerDown(point))?;
        self.route(InputEvent::PointerUp(point))?;
        if !self.control.is_open() {
            return Err("Native pointer click did not open the profile Select".into());
        }
        Ok(())
    }

    fn measurements(&self) -> Result<Vec<Value>> {
        self.ui
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let layout = self.ui.tree.layout(node.layout_id)?;
                let content_width = node.rect.w
                    - layout.padding.left
                    - layout.padding.right
                    - layout.border.left
                    - layout.border.right;
                let content_height = node.rect.h
                    - layout.padding.top
                    - layout.padding.bottom
                    - layout.border.top
                    - layout.border.bottom;
                let text_width = node.text.as_ref().map(|text| text.layout.width());
                let text_height = node.text.as_ref().map(|text| text.layout.height());
                Ok(json!({
                    "id":node.element.id,
                    "shown":self.ui.shown(index),
                    "rect":node.rect,
                    "clip":node.clip,
                    "visibleRect":node.rect.intersection(node.clip),
                    "contentWidth":content_width,
                    "contentHeight":content_height,
                    "text":node.text.as_ref().map(|text|text.value.as_str()),
                    "textWidth":text_width,
                    "textHeight":text_height,
                    "widthOverflow":text_width.is_some_and(|width|width>content_width+0.5),
                    "heightOverflow":text_height.is_some_and(|height|height>content_height+0.5)
                }))
            })
            .collect()
    }

    fn capture(
        &mut self,
        output: &Path,
        theme: &str,
        stage: &str,
        renderer: &mut Renderer,
        gpu: &mut NativeUi,
    ) -> Result<()> {
        let work = self.ui.frame(&mut self.text)?;
        let idle = self.ui.frame(&mut self.text)?;
        renderer.render(
            gpu,
            &self.ui.display,
            &self.text.assets,
            PIXELS,
            TextMode::Raster,
        )?;
        let stem = format!("{theme}-{stage}");
        gpu.pixels()?.save(output.join(format!("{stem}.png")))?;
        let identity = |id: Option<u64>| {
            id.map(|id|json!({"stableId":id,"id":self.ui.by_id(id).map(|index|self.ui.nodes[index].element.id.as_str())}))
        };
        let report = json!({
            "renderer":"NativeUi/windowless",
            "theme":theme,
            "stage":stage,
            "logicalSize":SIZE,
            "pixelSize":PIXELS,
            "scale":1,
            "requestedControlWidth":110,
            "requestedCaptionWidth":74,
            "optionCount":12,
            "fonts":"TextSystem::bundled: Noto Sans + Noto Naskh Arabic",
            "sourceCheckpoint":std::env::var("ARA_EVIDENCE_SOURCE").ok(),
            "binary":std::env::current_exe()?.display().to_string(),
            "focus":identity(self.input.focused()),
            "capture":identity(self.input.captured()),
            "open":self.control.is_open(),
            "query":self.control.query(),
            "selected":self.control.selected(),
            "measurements":self.measurements()?,
            "frameWork":{"layout":work.layout_passes,"shape":work.shaped,"paint":work.paint_nodes},
            "idleWork":{"layout":idle.layout_passes,"shape":idle.shaped,"paint":idle.paint_nodes},
            "ui":self.ui.inspect()
        });
        std::fs::write(
            output.join(format!("{stem}.json")),
            ara_json::to_canonical_string(&report),
        )?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("Pass an explicit isolated output directory")?;
    std::fs::create_dir_all(&output)?;
    // The shared presentation path selects supported hardware without an SDL window.
    let device = open_presentation(&[])?;
    // SAFETY: The presentation device outlives the adapter; this thread owns its queue.
    let mut gpu = unsafe { NativeUi::new(&device)? };
    let mut renderer = Renderer::default();
    for (theme, name) in [(EditorTheme::Dark, "dark"), (EditorTheme::Light, "light")] {
        let mut surface = Surface::new(theme)?;
        surface.capture(&output, name, "closed", &mut renderer, &mut gpu)?;
        surface.open()?;
        surface.capture(&output, name, "open", &mut renderer, &mut gpu)?;
    }
    Ok(())
}
