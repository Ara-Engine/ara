#![allow(dead_code)]
use ara_ui::{
    Result, Ui,
    docking::{Axis, DockLayout, DockNode},
    source::stable_id,
    theme::EDITOR_CSS,
    virtualize::{TreeItem, VirtualTable, VirtualTree, VirtualView},
};
use ara_ui_render::{
    CpuGpu,
    atlas::TextMode,
    gpu::{Image, Renderer},
};
use ara_ui_text::{Clipboard, EditEvent, TextSystem};
use std::path::Path;
#[derive(Default)]
pub struct Clip(pub String);
impl Clipboard for Clip {
    fn read(&mut self) -> Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, s: &str) -> Result<()> {
        self.0 = s.into();
        Ok(())
    }
}
pub fn text() -> Result<TextSystem> {
    TextSystem::new(
        [
            include_bytes!("../../../ara-ui-text/fonts/NotoSans.ttf").to_vec(),
            include_bytes!("../../../ara-ui-text/fonts/NotoNaskhArabic.ttf").to_vec(),
            include_bytes!("../../tests/fonts/NotoSansJP.ttf").to_vec(),
            include_bytes!("../../tests/fonts/NotoSansKR.ttf").to_vec(),
            include_bytes!("../../tests/fonts/NotoSansSC.ttf").to_vec(),
            include_bytes!("../../tests/fonts/NotoEmoji.ttf").to_vec(),
            include_bytes!("../../tests/fonts/NotoColorEmoji.ttf").to_vec(),
        ],
        "'Noto Sans', 'Noto Naskh Arabic', 'Noto Sans JP', 'Noto Sans KR', 'Noto Sans SC', 'Noto Color Emoji', 'Noto Emoji'",
    )
}
pub fn inspector() -> Result<Ui> {
    Ui::editor(
        r#"<ui id="inspector">
<panel id="header" style="gap:2px"><text id="title" class="heading" text="Inspector" /><text id="entity" class="muted" text="Navigation core · #core-07" /></panel>
<panel id="identity" class="card"><text id="type" text="Transform" /><text id="hint" class="muted" text="Local values relative to the parent" />
<panel id="r1" class="row"><text id="l1" class="muted" text="Position / m" /><field id="position" text="12.0, 0.0, −3.5" description="Local position relative to the parent." /></panel>
<panel id="r2" class="row"><text id="l2" class="muted" text="Rotation / deg" /><field id="rotation" text="0.0, 90.0, 0.0" /></panel>
<panel id="r3" class="row"><text id="l3" class="muted" text="Scale" /><field id="scale" text="1.0, 1.0, 1.0" /></panel></panel>
<panel id="gameplay" class="card"><text id="state-title" text="Salvage component" /><panel id="r4" class="row"><text id="l4" class="muted" text="Mass / kg" /><field id="mass" text="4.25" /></panel><toggle id="enabled" text="Enabled" checked="true" /><text id="health-title" class="muted" text="Integrity" /><slider id="health" value="0.86" /></panel>
<button id="reset" text="Reset selected overrides" action="ResetOverrides" /><text id="provenance" class="muted" text="Values: local override / prefab / default" />
</ui>"#,
        EDITOR_CSS,
        [480.0, 560.0],
    )
}
fn area(id: &str, tabs: &[&str]) -> DockNode {
    DockNode::Area {
        id: id.into(),
        tabs: tabs.iter().map(|s| (*s).into()).collect(),
        active: 0,
    }
}
pub fn dock() -> Result<Ui> {
    let layout = DockLayout::new(DockNode::Split {
        axis: Axis::Horizontal,
        ratio: 0.69,
        first: Box::new(DockNode::Split {
            axis: Axis::Vertical,
            ratio: 0.72,
            first: Box::new(area("scene", &["Scene", "Game"])),
            second: Box::new(area("console", &["Console", "Assets"])),
        }),
        second: Box::new(area("inspector", &["Inspector"])),
    })?;
    let content=layout.markup().replace("<text id=\"label-Scene\" class=\"muted\" text=\"Scene\" />",r#"<panel id="viewport" style="flex-grow:1; background:var(--background); border-radius:var(--radius-sm); padding:16px"><text id="scene-title" class="muted" text="OUTPOST / scene view" /><text id="scene-subtitle" text="Navigation core" /><text id="scene-caption" class="muted" text="Perspective · Lit · Local" /></panel>"#).replace("<text id=\"label-Console\" class=\"muted\" text=\"Console\" />",r#"<text id="log-title" class="muted" text="3 messages · All levels" /><text id="log-1" text="Scene loaded: outpost/landing-bay" /><text id="log-2" text="Local session · fixed tick 60 Hz" /><text id="log-3" class="muted" text="No problems found" />"#).replace("<text id=\"label-Inspector\" class=\"muted\" text=\"Inspector\" />",r#"<text id="component" text="Transform" /><text id="position-label" class="muted" text="Position / m" /><field id="position" text="12.0, 0.0, −3.5" /><text id="rotation-label" class="muted" text="Rotation / deg" /><field id="rotation" text="0.0, 90.0, 0.0" /><toggle id="enabled" text="Enabled" checked="true" /><text id="property-help" class="muted" text="Local override · prefab instance" />"#);
    Ui::editor(
        &format!(
            "<ui id='dock-root'><text id='project-title' text='Ara / Outpost' />{content}</ui>"
        ),
        EDITOR_CSS,
        [1100.0, 650.0],
    )
}
pub fn tree_model() -> Result<VirtualTree> {
    VirtualTree::new(
        (0..100_000)
            .map(|i| TreeItem {
                key: i as u64,
                parent: if i % 1000 == 0 {
                    None
                } else {
                    Some(i / 1000 * 1000)
                },
                text: format!(
                    "{} {i:06}",
                    if i % 1000 == 0 {
                        "v Sector"
                    } else {
                        "  Entity"
                    }
                ),
                expanded: true,
            })
            .collect(),
    )
}
pub fn tree() -> Result<(Ui, VirtualView, VirtualTree)> {
    let mut ui = Ui::editor(
        "<ui id='hierarchy'><text id='title' text='Hierarchy' /><text id='count' class='muted' text='100,000 entities · keyed virtual tree' /><scroll id='viewport' style='height:468px' /></ui>",
        EDITOR_CSS,
        [480.0, 560.0],
    )?;
    let parent = ui.find("viewport").ok_or("missing viewport")?;
    let view = VirtualView::mount(&mut ui, parent, 26.0, 468.0, vec![420.0])?;
    Ok((ui, view, tree_model()?))
}
pub fn text_input() -> Result<Ui> {
    let mut ui = Ui::editor(
        "<ui id='editing'><text id='title' class='heading' text='Text input / editing session' /><panel id='fields' class='card'><text id='label' class='muted' text='Display name · Arabic and English' /><field id='name' text='Outpost' action='Rename' /><text id='composition-label' class='muted' text='Japanese IME · uncommitted preedit' /><field id='composition' text='Mission: ' /><text id='hint' class='muted' text='Selection, clipboard, caret and undo share one edit state.' /></panel><button id='save' text='Save changes' action='Save' /></ui>",
        EDITOR_CSS,
        [640.0, 330.0],
    )?;
    let mut clip = Clip::default();
    let name = ui.find("name").ok_or("missing field")?;
    ui.edit(name, EditEvent::SelectAll, &mut clip)?;
    ui.edit(
        name,
        EditEvent::Insert("المهمة Outpost 42".into()),
        &mut clip,
    )?;
    ui.edit(name, EditEvent::Undo, &mut clip)?;
    ui.edit(name, EditEvent::Redo, &mut clip)?;
    ui.edit(
        name,
        EditEvent::Select {
            anchor: 13,
            caret: 23,
        },
        &mut clip,
    )?;
    let mut s = ui.nodes[name].state;
    s.focus = true;
    ui.set_state(name, s);
    let ime = ui.find("composition").ok_or("missing composition")?;
    ui.edit(ime, EditEvent::ImeStart, &mut clip)?;
    ui.edit(
        ime,
        EditEvent::ImePreedit {
            text: "にほんご".into(),
            selection: 0..12,
        },
        &mut clip,
    )?;
    let mut s = ui.nodes[ime].state;
    s.focus = true;
    ui.set_state(ime, s);
    Ok(ui)
}
pub fn pause() -> Result<Ui> {
    Ui::editor(
        "<ui id='pause'><panel id='menu' class='card' style='gap:16px; padding:28px'><text id='kicker' class='muted' text='OUTPOST / LOCAL SESSION' /><text id='title' style='font-size:28px' text='Expedition paused' /><text id='subtitle' class='muted' text='The landing bay is secure.' /><text id='health' text='Integrity 86 / 100' /><slider id='integrity' value='0.86' /><button id='resume' class='selected' text='Resume expedition' action='Resume' /><button id='settings' text='Settings' action='Settings' /><button id='exit' text='Return to base' action='Exit' /><text id='arabic' style='font-size:20px' text='استئناف الرحلة' /></panel></ui>",
        EDITOR_CSS,
        [520.0, 470.0],
    )
}
pub fn world() -> Result<Ui> {
    Ui::editor(
        "<ui id='world'><panel id='panel' class='card' style='gap:12px; padding:24px'><image id='icon' src='navigation-icon' style='width:32px; height:32px' /><text id='title' style='font-size:24px' text='Navigation core' /><text id='status' class='muted' text='Power available · Access granted' /><text id='arabic' style='font-size:22px' text='المحطة جاهزة للانطلاق' /><text id='japanese' text='航行準備完了' /><button id='interact' class='selected' text='E / Initialize navigation' action='Initialize' /></panel></ui>",
        EDITOR_CSS,
        [640.0, 360.0],
    )
}
pub fn text_lab() -> Result<Ui> {
    let css = format!(
        "{EDITOR_CSS}\n.title {{font-size:28px;}} .eyebrow {{font-size:12px; color:var(--muted-foreground);}} .text-sample {{font-size:24px; background:var(--card); padding:16px; border-radius:var(--radius-md);}} .preedit {{font-size:22px; height:44px;}} "
    );
    let mut ui = Ui::editor(
        include_str!("../../stories/text.ui"),
        &css,
        [1000.0, 1100.0],
    )?;
    let field = ui.find("ime-field").ok_or("IME field")?;
    ui.set_text(field, "")?;
    let mut clipboard = Clip::default();
    ui.edit(field, EditEvent::ImeStart, &mut clipboard)?;
    ui.edit(
        field,
        EditEvent::ImePreedit {
            text: "\u{306b}\u{307b}\u{3093}\u{3054}".into(),
            selection: 0..12,
        },
        &mut clipboard,
    )?;
    let mut state = ui.nodes[field].state;
    state.focus = true;
    ui.set_state(field, state);
    Ok(ui)
}
pub fn table() -> Result<(Ui, VirtualView, VirtualTable)> {
    let mut ui = Ui::editor(
        "<ui id='assets'><text id='title' text='Assets' /><panel id='columns' style='flex-direction:row; gap:0px'><text id='name-col' class='muted' text='Name' style='width:320px' /><text id='type-col' class='muted' text='Type' style='width:180px' /><text id='size-col' class='muted' text='Size' style='width:120px' /></panel><scroll id='viewport' style='height:260px' /></ui>",
        EDITOR_CSS,
        [680.0, 350.0],
    )?;
    let parent = ui.find("viewport").ok_or("missing table viewport")?;
    let view = VirtualView::mount(&mut ui, parent, 26.0, 260.0, vec![320.0, 180.0, 120.0])?;
    let rows = (0..10_000)
        .map(|i| {
            (
                i,
                vec![
                    format!("navigation-core-{i:05}"),
                    "Material".into(),
                    format!("{} KB", i % 400 + 1),
                ],
            )
        })
        .collect();
    Ok((
        ui,
        view,
        VirtualTable {
            rows,
            widths: vec![320.0, 180.0, 120.0],
        },
    ))
}
pub fn render(ui: &mut Ui, text: &mut TextSystem, mode: TextMode) -> Result<(Renderer, CpuGpu)> {
    ui.frame(text)?;
    let mut renderer = Renderer::default();
    let image = Image {
        width: 8,
        height: 8,
        rgba: (0..64)
            .flat_map(|i| {
                if (i / 8 + i % 8) % 2 == 0 {
                    [52, 107, 241, 255]
                } else {
                    [25, 25, 25, 255]
                }
            })
            .collect(),
    };
    renderer.image(stable_id("navigation-icon") as u32 & 0x7fff_ffff, image)?;
    let mut cpu = CpuGpu::default();
    renderer.render(
        &mut cpu,
        &ui.display,
        &text.assets,
        [ui.size[0] as u32, ui.size[1] as u32],
        mode,
    )?;
    Ok((renderer, cpu))
}
pub fn capture(
    name: &str,
    ui: &mut Ui,
    text: &mut TextSystem,
    out: &Path,
    mode: TextMode,
) -> Result<()> {
    let (renderer, cpu) = render(ui, text, mode)?;
    cpu.save(&out.join(format!("{name}.png")))?;
    let inspection = serde_json::json!({"ui":ui.inspect(),"batches":renderer.batches,"atlas_bytes":renderer.atlas.bytes()});
    std::fs::write(
        out.join(format!("{name}.json")),
        ara_json::to_canonical_string(&inspection),
    )?;
    Ok(())
}
