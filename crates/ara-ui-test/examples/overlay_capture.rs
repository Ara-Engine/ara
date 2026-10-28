//! Windowless production-GPU overlay evidence; every artifact uses the required output path.
use ara_editor_gpu::{NativeUi, open_presentation};
use ara_ui::{
    Result, Ui,
    dialogs::{DialogOptions, Dialogs},
    input::{InputEvent, InputState},
    paint::Box2,
    popover::{HoverDelay, MAX_WIDTH, SHOW_DELAY_MS},
    source::stable_id,
    theme::{EDITOR_CSS, EditorTheme},
};
use ara_ui_render::{atlas::TextMode, gpu::Renderer};
use ara_ui_text::{Clipboard, EditEvent, TextSystem};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SIZE: [f32; 2] = [640., 520.];
const PIXELS: [u32; 2] = [640, 520];
const STYLES: &str = "
ui { padding:24px; gap:12px; background:var(--background); color:var(--foreground); }
.heading { font-size:20px; }
.caption { font-size:12px; color:var(--muted-foreground); white-space:normal; }
.anchor { position:absolute; width:144px; height:auto; min-height:32px; right:12px; }
.bubble { padding:12px; gap:8px; background:var(--popover); color:var(--popover-foreground); border-width:1px; border-color:var(--border); border-radius:8px; }
.bubble-copy { white-space:normal; }
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
    ui: Ui,
    input: InputState,
    text: TextSystem,
    dialogs: Dialogs,
    clipboard: Clip,
    tooltip: bool,
    popover: bool,
    events: Vec<Value>,
    checks: Vec<Value>,
    captures: Vec<String>,
}
impl Surface {
    fn new(theme: EditorTheme) -> Result<Self> {
        let mut surface = Self {
            ui: Ui::editor(
                "<ui id='root' />",
                &format!("{EDITOR_CSS}{}{STYLES}", ara_ui::dialogs::CSS),
                SIZE,
            )?,
            input: InputState::default(),
            text: TextSystem::bundled()?,
            dialogs: Dialogs::new("overlay"),
            clipboard: Clip::default(),
            tooltip: false,
            popover: false,
            events: Vec::new(),
            checks: Vec::new(),
            captures: Vec::new(),
        };
        surface.ui.switch_theme(theme)?;
        surface.mount()?;
        surface.input.push(
            &mut surface.ui,
            stable_id("root"),
            Some(stable_id("outside")),
            false,
        )?;
        Ok(surface)
    }
    fn mount(&mut self) -> Result<()> {
        let mut markup = String::from(
            "<ui id='root'><text id='heading' class='heading' text='Shared native overlays' /><text id='caption' class='caption' text='Production GPU · canonical theme · platform-neutral input events' /><button id='outside' text='Return focus here' action='outside' style='width:180px' /><button id='popover-anchor' class='anchor' text='Recovery menu' style='top:112px' /><button id='tooltip-anchor' class='anchor' text='Property help' style='bottom:12px' />",
        );
        if self.popover {
            markup.push_str("<popover id='edge-popover' class='bubble' anchor='popover-anchor' placement='right-start' offset='8'><text id='popover-title' class='bubble-copy' text='Recovery actions' /><text id='popover-copy' class='bubble-copy' text='Keep the current project open while reviewing these recovery actions. This description wraps inside the shared content width and the menu flips left when there is no room beside its anchor.' /><button id='popover-action' text='Open details' action='details' /></popover>");
        }
        if self.tooltip {
            markup.push_str("<tooltip id='edge-tooltip' class='bubble' anchor='tooltip-anchor' placement='bottom-start' offset='8'><text id='tooltip-copy' class='bubble-copy' text='This property accepts a scene reference. Drag an entity here or choose it from the picker. Long explanations wrap to the shared maximum width, and the hint moves above an anchor near the bottom edge.' /></tooltip>");
        }
        markup.push_str(&self.dialogs.markup(SIZE));
        markup.push_str("</ui>");
        self.ui.reconcile(&markup)?;
        self.ui.frame(&mut self.text)?;
        self.dialogs.sync(&mut self.ui, &mut self.input)?;
        self.ui.frame(&mut self.text)?;
        Ok(())
    }
    fn event(&mut self, label: &str, event: InputEvent) -> Result<()> {
        let before = self.focus().map(str::to_owned);
        let modal = self.dialogs.is_open();
        if modal {
            self.dialogs
                .event(&mut self.ui, &mut self.input, event, &mut self.clipboard)?;
        } else {
            self.input.event(&mut self.ui, event, &mut self.clipboard)?;
        }
        self.mount()?;
        self.events.push(json!({"event":label,"modalConsumed":modal,"focusBefore":before,"focusAfter":self.focus(),"dialogsAfter":self.dialogs.len()}));
        Ok(())
    }
    fn focus(&self) -> Option<&str> {
        self.input.focused().and_then(|id| self.ui.authored_id(id))
    }
    fn check(&mut self, name: &str, passed: bool, observed: Value) {
        self.checks
            .push(json!({"check":name,"passed":passed,"observed":observed}));
    }
    fn focus_is(&mut self, name: &str, expected: &str) {
        self.check(
            name,
            self.focus() == Some(expected),
            json!({"expected":expected,"actual":self.focus()}),
        );
    }
    fn rect(&self, id: &str) -> Result<Box2> {
        self.ui
            .find(id)
            .map(|i| self.ui.nodes[i].rect)
            .ok_or_else(|| format!("Overlay evidence node is missing: {id}").into())
    }
    fn scroll(&self, id: &str) -> Result<[f32; 2]> {
        self.ui
            .find(id)
            .and_then(|i| self.ui.scroll_position(i))
            .ok_or_else(|| format!("Overlay evidence scroll is missing: {id}").into())
    }
    fn capture(
        &mut self,
        output: &Path,
        stem: &str,
        renderer: &mut Renderer,
        gpu: &mut NativeUi,
    ) -> Result<()> {
        let work = self.ui.frame(&mut self.text)?;
        let idle = self.ui.frame(&mut self.text)?;
        self.check(
            "idle frame does no layout, shaping or painting",
            idle.layout_passes == 0 && idle.shaped == 0 && idle.paint_nodes == 0,
            json!(idle),
        );
        let surfaces = self.ui.nodes.iter().enumerate().filter(|(index,node)|self.ui.shown(*index)&&(node.element.id.ends_with("-body")||matches!(node.element.tag.as_str(),"tooltip"|"popover"))).map(|(_,node)| {
            let rect=node.rect;
            json!({"id":node.element.id,"rect":rect,"clip":node.clip,"widthBounded":rect.w>=24.&&rect.w<=MAX_WIDTH+0.5,
                "insideViewport":rect.x>=-0.5&&rect.y>=-0.5&&rect.x+rect.w<=SIZE[0]+0.5&&rect.y+rect.h<=SIZE[1]+0.5})
        }).collect::<Vec<_>>();
        for surface in &surfaces {
            self.check(
                "overlay is bounded and inside viewport",
                surface["widthBounded"] == true && surface["insideViewport"] == true,
                surface.clone(),
            );
        }
        renderer.render(
            gpu,
            &self.ui.display,
            &self.text.assets,
            PIXELS,
            TextMode::Raster,
        )?;
        gpu.pixels()?.save(output.join(format!("{stem}.png")))?;
        let paragraphs=self.ui.nodes.iter().enumerate().filter(|(index,_)|self.ui.shown(*index)).filter_map(|(_,node)|node.text.as_ref().map(|text|json!({"id":node.element.id,"text":text.value,"rect":node.rect,"clip":node.clip,"lines":text.layout.len(),"textHeight":text.layout.height(),"textFitsLayoutBox":text.layout.height()<=node.rect.h+0.5}))).collect::<Vec<_>>();
        for paragraph in &paragraphs {
            self.check(
                "paragraph height fits its measured layout box",
                paragraph["textFitsLayoutBox"] == true,
                paragraph.clone(),
            );
        }
        let report = json!({"renderer":"NativeUi/windowless","logicalSize":SIZE,"physicalSize":PIXELS,"stage":stem,
            "focus":self.focus(),"focusGraph":self.input.focus_graph(&self.ui),"capture":self.input.captured(),"dialogs":self.dialogs.len(),
            "frameWork":work,"idleWork":idle,"bounds":surfaces,"paragraphs":paragraphs,"events":self.events,"checks":self.checks,"ui":self.ui.inspect()});
        std::fs::write(
            output.join(format!("{stem}.json")),
            ara_json::to_canonical_string(&report),
        )?;
        self.captures.push(stem.into());
        Ok(())
    }
}

fn dialogs(
    surface: &mut Surface,
    output: &Path,
    theme: &str,
    renderer: &mut Renderer,
    gpu: &mut NativeUi,
) -> Result<()> {
    let mut alert=surface.dialogs.alert(DialogOptions::new("Import could not finish", "The source file is no longer available. Keep this project open, restore the source, and retry the import. Your authored records remain unchanged."));
    surface.mount()?;
    surface.focus_is("alert focuses its default action", "overlay-1-accept");
    surface.capture(output, &format!("{theme}-01-alert"), renderer, gpu)?;
    surface.event("Enter / Activate alert", InputEvent::Activate)?;
    let result = alert.try_take();
    surface.check(
        "Enter completes the alert",
        result == Some(()),
        json!({"completed":result.is_some()}),
    );
    surface.focus_is("alert restores outside focus", "outside");

    let mut prompt = surface.dialogs.prompt(
        DialogOptions::new(
            "Save layout",
            "Name this layout. The draft stays in the prompt while another confirmation is open.",
        ),
        "",
    );
    surface.mount()?;
    surface.focus_is(
        "prompt initially focuses its native text field",
        "overlay-2-field",
    );
    surface.event(
        "Insert text in prompt",
        InputEvent::Edit(EditEvent::Insert("Preview 4 layout".into())),
    )?;
    surface.capture(output, &format!("{theme}-02-prompt"), renderer, gpu)?;

    let mut options = DialogOptions::new(
        "Delete the saved layout?",
        "The saved layout will be removed. Cancel returns to the naming draft below without changing it.",
    );
    options.destructive = true;
    options.accept_label = "Delete layout".into();
    let mut confirm = surface.dialogs.confirm(options);
    surface.mount()?;
    surface.focus_is("destructive confirm defaults to Cancel", "overlay-3-cancel");
    let rejected = surface
        .input
        .focus(&mut surface.ui, stable_id("outside"))
        .err()
        .map(|error| error.to_string());
    surface.check(
        "modal rejects outside focus",
        rejected.is_some(),
        json!({"error":rejected}),
    );
    for _ in 0..6 {
        surface.event(
            "Tab within nested confirm",
            InputEvent::Tab { reverse: false },
        )?;
        let valid = matches!(
            surface.focus(),
            Some("overlay-3-cancel" | "overlay-3-accept")
        );
        surface.check(
            "Tab stays in top modal",
            valid,
            json!({"focus":surface.focus()}),
        );
    }
    let outside = surface.rect("outside")?;
    let point = [outside.x + outside.w * 0.5, outside.y + outside.h * 0.5];
    surface.event(
        "Pointer down on obscured outside button",
        InputEvent::PointerDown(point),
    )?;
    surface.event(
        "Pointer up on obscured outside button",
        InputEvent::PointerUp(point),
    )?;
    let leaked = surface
        .ui
        .drain_actions()
        .any(|action| action.name == "outside");
    surface.check(
        "backdrop blocks underlying activation and preserves stack",
        !leaked && surface.dialogs.len() == 2,
        json!({"underlyingAction":leaked,"dialogs":surface.dialogs.len()}),
    );
    surface.capture(output, &format!("{theme}-03-nested-confirm"), renderer, gpu)?;
    surface.event("Escape / Cancel nested confirm", InputEvent::Cancel)?;
    let result = confirm.try_take();
    surface.check(
        "Escape cancels only nested confirmation",
        result == Some(false) && surface.dialogs.len() == 1,
        json!({"result":result,"dialogs":surface.dialogs.len()}),
    );
    surface.focus_is("Escape restores prompt field", "overlay-2-field");
    surface.capture(
        output,
        &format!("{theme}-04-restored-prompt"),
        renderer,
        gpu,
    )?;
    surface.event("Enter / Activate prompt", InputEvent::Activate)?;
    let result = prompt.try_take();
    surface.check(
        "Enter accepts preserved text draft",
        result == Some(Some("Preview 4 layout".into())),
        json!({"result":result}),
    );
    surface.focus_is("prompt completion restores outside focus", "outside");

    let labels = || {
        vec![
            "Rename — preserve the existing authored asset".into(),
            "Replace — use the selected source file".into(),
            "Skip — keep this file unchanged".into(),
        ]
    };
    let mut choice=surface.dialogs.choose(DialogOptions::new("Existing destination", "Choose how to resolve this known conflict. No file operation runs in this primitive capture."),labels(),0);
    surface.mount()?;
    surface.focus_is("choice starts on safe Rename default", "overlay-4-choice-0");
    surface.event(
        "Tab to second conflict choice",
        InputEvent::Tab { reverse: false },
    )?;
    surface.focus_is("Tab reaches Replace choice", "overlay-4-choice-1");
    surface.capture(output, &format!("{theme}-05-choice"), renderer, gpu)?;
    surface.event(
        "Enter / Activate focused Replace choice",
        InputEvent::Activate,
    )?;
    let result = choice.try_take();
    surface.check(
        "choice returns explicitly focused index",
        result == Some(Some(1)),
        json!({"result":result}),
    );
    surface.focus_is("choice restores outside focus", "outside");
    let mut cancelled = surface.dialogs.choose(
        DialogOptions::new(
            "Another conflict",
            "Escape leaves this conflict unresolved.",
        ),
        labels(),
        0,
    );
    surface.mount()?;
    surface.event("Escape / Cancel conflict choice", InputEvent::Cancel)?;
    let result = cancelled.try_take();
    surface.check(
        "Escape returns cancelled choice",
        result == Some(None),
        json!({"completed":result.is_some(),"result":result}),
    );
    surface.focus_is("choice cancellation restores outside focus", "outside");

    let notes=(1..=30).map(|n|format!("Recovery note {n}: preserve the current project, inspect the diagnostic, then retry only the failed operation.\n")).collect::<String>();
    let mut long = surface
        .dialogs
        .alert(DialogOptions::new("Recovery notes", notes));
    surface.mount()?;
    surface.capture(output, &format!("{theme}-06-long-notes-top"), renderer, gpu)?;
    let before = surface.scroll("overlay-6-scroll")?;
    let scroll = surface.rect("overlay-6-scroll")?;
    let position = [scroll.x + scroll.w * 0.5, scroll.y + scroll.h * 0.5];
    surface.event(
        "Wheel through long recovery notes",
        InputEvent::Wheel {
            position,
            delta: [0., 100_000.],
        },
    )?;
    let after = surface.scroll("overlay-6-scroll")?;
    surface.check(
        "long notes scroll through native wheel input",
        after[1] > before[1],
        json!({"before":before,"after":after}),
    );
    let action = surface.rect("overlay-6-accept")?;
    let index = surface
        .ui
        .find("overlay-6-accept")
        .ok_or("Long-note action is missing")?;
    let centre = [action.x + action.w * 0.5, action.y + action.h * 0.5];
    surface.check(
        "long-note action remains visible and hit-testable",
        surface.ui.nodes[index].clip.contains(centre[0], centre[1])
            && surface.ui.hit_test(centre[0], centre[1]) == Some(index),
        json!({"action":action,"clip":surface.ui.nodes[index].clip}),
    );
    surface.capture(
        output,
        &format!("{theme}-07-long-notes-scrolled"),
        renderer,
        gpu,
    )?;
    surface.event("Enter / Activate long-note alert", InputEvent::Activate)?;
    let result = long.try_take();
    surface.check(
        "long-note alert completes",
        result == Some(()),
        json!({"completed":result.is_some()}),
    );
    surface.focus_is("long-note alert restores outside focus", "outside");
    Ok(())
}

fn edges(
    surface: &mut Surface,
    output: &Path,
    theme: &str,
    renderer: &mut Renderer,
    gpu: &mut NativeUi,
) -> Result<()> {
    let mut delay = HoverDelay::default();
    let anchor = surface.rect("tooltip-anchor")?;
    let point = [anchor.x + anchor.w * 0.5, anchor.y + anchor.h * 0.5];
    surface.event(
        "Pointer hover over tooltip anchor",
        InputEvent::PointerMove(point),
    )?;
    let target = surface
        .ui
        .hit_test(point[0], point[1])
        .map(|index| stable_id(&surface.ui.nodes[index].element.id));
    surface.check(
        "hover target is the actual hit-tested anchor",
        target == Some(stable_id("tooltip-anchor")),
        json!({"actual":target,"expected":stable_id("tooltip-anchor")}),
    );
    let before = delay.update(target, 0);
    let early = delay.update(target, SHOW_DELAY_MS - 1);
    let shown = delay.update(target, SHOW_DELAY_MS);
    surface.check(
        "tooltip respects shared deterministic hover delay",
        !before && !early && shown,
        json!({"milliseconds":[0,SHOW_DELAY_MS-1,SHOW_DELAY_MS],"shown":[before,early,shown]}),
    );
    surface.tooltip = shown;
    surface.popover = true;
    surface.mount()?;
    surface.focus_is("tooltip and nonmodal menu markup preserve focus", "outside");
    for (id, anchor, copy, vertical) in [
        ("edge-tooltip", "tooltip-anchor", "tooltip-copy", true),
        ("edge-popover", "popover-anchor", "popover-copy", false),
    ] {
        let rect = surface.rect(id)?;
        let anchor = surface.rect(anchor)?;
        let flipped = if vertical {
            rect.y + rect.h <= anchor.y - 7.5
        } else {
            rect.x + rect.w <= anchor.x - 7.5
        };
        surface.check("edge placement flips away from unavailable side",flipped,json!({"surface":id,"rect":rect,"anchor":anchor,"preferred":if vertical {"bottom-start"}else{"right-start"}}));
        let copy = surface
            .ui
            .find(copy)
            .and_then(|i| surface.ui.nodes[i].text.as_ref())
            .ok_or("Wrapped overlay copy is missing")?;
        let lines = copy.layout.len();
        let height = copy.layout.height();
        surface.check(
            "long overlay text actually wraps",
            lines > 1,
            json!({"surface":id,"lines":lines,"textHeight":height}),
        );
    }
    surface.capture(
        output,
        &format!("{theme}-08-edge-flipped-overlays"),
        renderer,
        gpu,
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or("Pass one explicit isolated output directory")?;
    if args.next().is_some() {
        return Err("Pass only the explicit output directory".into());
    }
    std::fs::create_dir_all(&output)?;
    let device = open_presentation(&[])?;
    // SAFETY: This thread owns the queue, and the presentation device outlives the adapter.
    let mut gpu = unsafe { NativeUi::new(&device)? };
    let mut renderer = Renderer::default();
    let mut reports = Vec::new();
    let mut failed = false;
    for (theme, name) in [(EditorTheme::Dark, "dark"), (EditorTheme::Light, "light")] {
        let mut surface = Surface::new(theme)?;
        let result = dialogs(&mut surface, &output, name, &mut renderer, &mut gpu)
            .and_then(|()| edges(&mut surface, &output, name, &mut renderer, &mut gpu));
        let failures = surface
            .checks
            .iter()
            .filter(|check| check["passed"] != true)
            .count();
        failed |= failures > 0 || result.is_err();
        reports.push(json!({"theme":name,"completed":result.is_ok(),"error":result.err().map(|error|error.to_string()),"checks":surface.checks,"failedChecks":failures,"events":surface.events,"captures":surface.captures}));
        std::fs::write(
            output.join("overlays.json"),
            ara_json::to_canonical_string(
                &json!({"renderer":"NativeUi/windowless","completed":reports.len()==2&&reports.iter().all(|report|report["completed"]==true),"ok":!failed,"themes":reports}),
            ),
        )?;
    }
    if failed {
        return Err("Overlay evidence contains failed checks; inspect overlays.json".into());
    }
    Ok(())
}
