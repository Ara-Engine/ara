use super::*;
use crate::{binding::Action, theme::EDITOR_CSS};
use ara_ui_text::{Clipboard, EditEvent, TextSystem};

struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

fn mount(
    select: &mut Select<usize>,
    ui: &mut Ui,
    input: &mut InputState,
    text: &mut TextSystem,
) -> Result<()> {
    ui.reconcile(&format!("<ui id='root'><button id='outside' text='Outside' /><panel id='form' style='position:absolute;left:8px;top:8px;max-width:300px'>{}</panel>{}</ui>", select.control_markup(), select.popover_markup()))?;
    ui.frame(text)?;
    select.sync(ui, input)?;
    ui.frame(text)?;
    Ok(())
}

fn surface() -> Result<(Ui, InputState, TextSystem)> {
    surface_with_styles([320., 300.], "")
}

fn surface_with_styles(size: [f32; 2], host_styles: &str) -> Result<(Ui, InputState, TextSystem)> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='outside' text='Outside' /></ui>",
        &format!("{EDITOR_CSS}{host_styles}{CSS}"),
        size,
    )?;
    ui.frame(&mut text)?;
    let mut input = InputState::default();
    input.push(
        &mut ui,
        stable_id("root"),
        Some(stable_id("outside")),
        false,
    )?;
    Ok((ui, input, text))
}

// Host panel styles must not replace a select's intrinsic list basis or wrapped row height.
const HOST_RESETS: &str = "button { height:22px; } dropdown { height:22px; } scroll { flex-basis:0px; min-height:0px; min-width:0px; }";

#[test]
fn closed_select_has_native_chevron_reserved_caption_space_and_enabled_hover_focus_disabled_states()
-> Result<()> {
    use crate::{cursor::Cursor, paint::Draw, theme::EditorTheme};
    for theme in [EditorTheme::Dark, EditorTheme::Light] {
        let (mut ui, mut input, mut text) = surface()?;
        ui.switch_theme(theme)?;
        let mut select = Select::new(
            "affordance",
            "Engine version",
            vec![SelectOption::new(0, "Preview 4 — إصدار المعاينة")],
            Some(0),
        );
        mount(&mut select, &mut ui, &mut input, &mut text)?;
        let index = ui.find(select.control_id()).expect("native select");
        let rect = ui.nodes[index].rect;
        let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        assert_eq!(ui.cursor(&input, point), Cursor::Pointer);
        assert_eq!(
            ui.nodes[index].computed.color,
            ui.sheet.color_token("--foreground").expect("foreground")
        );
        assert_eq!(
            ui.nodes[index].computed.background,
            ui.sheet
                .color_token("--background")
                .expect("enabled field fill")
        );
        let id = stable_id(select.control_id());
        let lines: Vec<_> = ui
            .display
            .commands
            .iter()
            .filter_map(|draw| {
                if let Draw::Line {
                    node,
                    from,
                    to,
                    color,
                    ..
                } = draw
                {
                    (*node == id).then_some((from, to, color))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            lines.len(),
            2,
            "chevron does not depend on font glyph coverage"
        );
        assert!(lines[0].0[1] < lines[0].1[1], "closed chevron points down");
        let layout = ui.tree.layout(ui.nodes[index].layout_id)?;
        let caption = ui.nodes[index].text.as_ref().expect("selected caption");
        let text_right = rect.x + layout.padding.left + layout.border.left + caption.layout.width();
        assert!(text_right <= rect.x + rect.w - layout.padding.right - layout.border.right + 0.5);
        assert!(
            lines[0].0[0] > rect.x + rect.w - layout.padding.right,
            "arrow is outside the caption area"
        );
        route(
            &mut select,
            &mut ui,
            &mut input,
            &mut text,
            InputEvent::PointerMove(point),
        )?;
        assert_eq!(
            ui.nodes[index].computed.background,
            ui.sheet.color_token("--hover-surface").expect("hover")
        );
        input.focus(&mut ui, id)?;
        ui.frame(&mut text)?;
        assert_eq!(
            ui.nodes[index].computed.border_color,
            ui.sheet.color_token("--ring").expect("focus ring")
        );
        let mut unavailable = SelectOption::new(0, "Engine not installed");
        unavailable.disabled = Some("Install the engine first".into());
        select.set_options(vec![unavailable]);
        mount(&mut select, &mut ui, &mut input, &mut text)?;
        assert!(ui.nodes[index].state.disabled);
        assert_eq!(
            ui.nodes[index].computed.color,
            ui.sheet
                .color_token("--muted-foreground")
                .expect("disabled text")
        );
        let rect = ui.nodes[index].rect;
        let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        assert_eq!(ui.cursor(&input, point), Cursor::NotAllowed);
        for event in [InputEvent::PointerDown(point), InputEvent::PointerUp(point)] {
            route(&mut select, &mut ui, &mut input, &mut text, event)?;
        }
        assert!(!select.is_open());
        assert_eq!(select.take_changed(), None);
        let idle = ui.frame(&mut text)?;
        assert_eq!(
            (idle.layout_passes, idle.shaped, idle.paint_nodes),
            (0, 0, 0)
        );
    }
    Ok(())
}

#[test]
fn empty_select_search_hint_is_painted_but_never_value_and_native_preedit_hides_it() -> Result<()> {
    use crate::{
        native_input::{Router, TextSource},
        paint::Draw,
    };
    let (mut ui, mut input, mut text) = surface()?;
    let mut select = Select::new(
        "hint",
        "Engine version",
        vec![
            SelectOption::new(0, "Preview 4"),
            SelectOption::new(1, "Nightly"),
        ],
        Some(0),
    );
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    let rect = ui.nodes[ui.find(select.control_id()).expect("control")].rect;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    for event in [InputEvent::PointerDown(point), InputEvent::PointerUp(point)] {
        route(&mut select, &mut ui, &mut input, &mut text, event)?;
    }
    let id = stable_id("hint-search");
    let index = ui.find("hint-search").expect("search");
    let layout = ui.tree.layout(ui.nodes[index].layout_id)?;
    let available_height = layout.size.height
        - layout.padding.top
        - layout.padding.bottom
        - layout.border.top
        - layout.border.bottom;
    let hint_height = ui.nodes[index]
        .placeholder
        .as_ref()
        .expect("hint paragraph")
        .layout
        .height();
    assert!(
        hint_height <= available_height + 0.5,
        "Search hint must fit inside the field content box: {hint_height} > {available_height}"
    );
    assert_eq!(input.focused(), Some(id));
    assert_eq!(select.query(), "");
    assert_eq!(
        ui.nodes[index]
            .placeholder
            .as_ref()
            .expect("real hint")
            .value,
        "Search…"
    );
    let muted = ui
        .sheet
        .color_token("--muted-foreground")
        .expect("hint color");
    let hint_glyphs = |ui: &Ui| {
        ui.display
            .commands
            .iter()
            .filter(
                |draw| matches!(draw,Draw::GlyphRun{node,color,..} if *node==id && *color==muted),
            )
            .count()
    };
    assert!(hint_glyphs(&ui) > 0);
    assert!(
        ui.display.glyphs.iter().all(|glyph| glyph.id != 0),
        "bundled Search ellipsis has coverage"
    );
    let bounds = ui.nodes[index].rect;
    assert_eq!(
        ui.hit_test(bounds.x + bounds.w * 0.5, bounds.y + bounds.h * 0.5),
        Some(index)
    );
    let value = |ui: &Ui| match &ui.widgets[&id] {
        crate::widgets::Widget::Input(input) => input.value().to_owned(),
        _ => panic!("search input"),
    };
    assert_eq!(value(&ui), "");
    let accessibility = crate::accessibility::accessibility(&ui, input.focused());
    let accessible = accessibility
        .nodes
        .iter()
        .find(|(node, _)| node.0 == id)
        .expect("accessible search");
    assert_eq!(
        accessible.1.value(),
        Some(""),
        "hint is not accessibility data"
    );
    // Native preedit goes straight into Ui, without changing Select::query or
    // requiring the host to regenerate markup.
    let mut router = Router::default();
    router.preedit(&mut ui, &mut input, TextSource::Sdl, "pré", 0, 3, &mut Clip)?;
    ui.frame(&mut text)?;
    assert_eq!(select.query(), "");
    assert_eq!(value(&ui), "");
    assert_eq!(hint_glyphs(&ui), 0);
    router.preedit(&mut ui, &mut input, TextSource::Sdl, "", 0, 0, &mut Clip)?;
    ui.frame(&mut text)?;
    assert!(hint_glyphs(&ui) > 0);
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::Edit(EditEvent::Insert("not present".into())),
    )?;
    assert_eq!(value(&ui), "not present");
    assert_eq!(hint_glyphs(&ui), 0);
    assert!(ui.find("hint-popover-empty").is_some());
    assert_eq!(select.selected(), Some(&0));
    select.key(&mut ui, &mut input, Key::Escape)?;
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    select.key(&mut ui, &mut input, Key::Enter)?;
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    assert_eq!(value(&ui), "");
    assert!(hint_glyphs(&ui) > 0);
    Ok(())
}

#[test]
fn placeholder_only_changes_repaint_without_layout_and_theme_switches_reuse_its_shaping()
-> Result<()> {
    use crate::theme::EditorTheme;
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><field id='field' placeholder='Search…' /></ui>",
        EDITOR_CSS,
        [300., 100.],
    )?;
    ui.frame(&mut text)?;
    let index = ui.find("field").expect("empty field");
    let rect = ui.nodes[index].rect;
    for theme in [EditorTheme::Light, EditorTheme::Dark] {
        ui.switch_theme(theme)?;
        let work = ui.frame(&mut text)?;
        assert_eq!((work.layout_passes, work.shaped), (0, 0));
        assert_eq!(ui.nodes[index].rect, rect);
        let idle = ui.frame(&mut text)?;
        assert_eq!(
            (idle.layout_passes, idle.shaped, idle.paint_nodes),
            (0, 0, 0)
        );
    }
    ui.reconcile("<ui id='root'><field id='field' placeholder='Filtrer les versions…' /></ui>")?;
    let work = ui.frame(&mut text)?;
    assert_eq!(work.layout_passes, 0);
    assert_eq!(work.shaped, 1);
    assert_eq!(ui.nodes[index].rect, rect);
    let mut input = InputState::default();
    input.push(&mut ui, stable_id("root"), Some(stable_id("field")), false)?;
    input.event(
        &mut ui,
        InputEvent::Edit(EditEvent::Insert("actual value".into())),
        &mut Clip,
    )?;
    ui.frame(&mut text)?;
    assert!(
        matches!(&ui.widgets[&stable_id("field")],crate::widgets::Widget::Input(input) if input.value()=="actual value")
    );
    Ok(())
}

#[test]
fn short_select_reopens_with_a_visible_list_and_accepts_a_real_pointer_choice_under_host_resets()
-> Result<()> {
    let (mut ui, mut input, mut text) = surface_with_styles([1100., 760.], HOST_RESETS)?;
    let options = ["Ara", "Unreal", "Unity", "Blender"]
        .iter()
        .enumerate()
        .map(|(value, label)| SelectOption::new(value, *label))
        .collect();
    let mut select = Select::new("preset", "Keymap preset", options, Some(0));
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    input.focus(&mut ui, stable_id(select.control_id()))?;
    select.key(&mut ui, &mut input, Key::Enter)?;
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    select.key(&mut ui, &mut input, Key::Down)?;
    select.key(&mut ui, &mut input, Key::Enter)?;
    assert_eq!(select.take_changed(), Some(1));
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    let control = ui.nodes[ui.find(select.control_id()).expect("select control")].rect;
    let point = [control.x + control.w * 0.5, control.y + control.h * 0.5];
    for event in [InputEvent::PointerDown(point), InputEvent::PointerUp(point)] {
        route(&mut select, &mut ui, &mut input, &mut text, event)?;
    }
    assert!(select.is_open());
    let list = &ui.nodes[ui.find("preset-popover-options").expect("option list")];
    assert!(
        list.rect.h > 0. && list.clip.h > 0.,
        "{:?} clipped by {:?}",
        list.rect,
        list.clip
    );
    let option = ui.find("preset-option-0-2").expect("Unity option");
    let row = &ui.nodes[option];
    assert!(
        row.rect.y >= row.clip.y - 0.5 && row.rect.y + row.rect.h <= row.clip.y + row.clip.h + 0.5,
        "{:?} clipped by {:?}",
        row.rect,
        row.clip
    );
    let point = [row.rect.x + row.rect.w * 0.5, row.rect.y + row.rect.h * 0.5];
    assert_eq!(ui.hit_test(point[0], point[1]), Some(option));
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::PointerDown(point),
    )?;
    assert_eq!(input.captured(), Some(stable_id("preset-option-0-2")));
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::PointerUp(point),
    )?;
    assert_eq!(select.take_changed(), Some(2));
    assert_eq!(select.selected(), Some(&2));
    assert!(!select.is_open());
    assert_eq!(input.focused(), Some(stable_id(select.control_id())));
    Ok(())
}

#[test]
fn hundred_wrapped_options_use_content_height_and_scroll_inside_the_popup_under_host_resets()
-> Result<()> {
    let (mut ui, mut input, mut text) = surface_with_styles([480., 480.], HOST_RESETS)?;
    let options=(0..100).map(|value|SelectOption::new(value,format!("Localized choice {value:03}: éclairage et matériaux, إعدادات العرض والتفاصيل, and a long explanation that wraps inside the native Select"))).collect();
    let mut select = Select::new("long", "Localized choice", options, Some(0));
    select.open("");
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    select.key(&mut ui, &mut input, Key::End)?;
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    let popup = &ui.nodes[ui.find("long-popover").expect("popup")];
    assert!(
        popup.rect.w <= crate::popover::MAX_WIDTH + 0.5
            && popup.rect.x >= 0.
            && popup.rect.y >= 0.
            && popup.rect.x + popup.rect.w <= 480.5
            && popup.rect.y + popup.rect.h <= 480.5,
        "{:?}",
        popup.rect
    );
    let list = ui.find("long-popover-options").expect("list");
    assert!(ui.nodes[list].rect.h > 0. && ui.nodes[list].clip.h > 0.);
    assert!(ui.scroll_position(list).expect("scroll view")[1] > 0.);
    for id in ["long-control", "long-option-0-99"] {
        let node = &ui.nodes[ui.find(id).expect("wrapping select text")];
        let paragraph = node.text.as_ref().expect("text");
        let layout = ui.tree.layout(node.layout_id)?;
        let available = layout.size.height
            - layout.padding.top
            - layout.padding.bottom
            - layout.border.top
            - layout.border.bottom;
        assert!(paragraph.layout.height() > 26., "long labels must wrap");
        assert!(
            paragraph.layout.height() <= available + 0.5,
            "{id}: {} > {available}",
            paragraph.layout.height()
        );
    }
    let option = &ui.nodes[ui.find("long-option-0-99").expect("final option")];
    assert!(
        option.rect.y >= option.clip.y - 0.5
            && option.rect.y + option.rect.h <= option.clip.y + option.clip.h + 0.5,
        "{:?} clipped by {:?}",
        option.rect,
        option.clip
    );
    assert_eq!(
        ui.frame(&mut text)?.layout_passes,
        0,
        "content fitting must settle before idle"
    );
    Ok(())
}

fn route(
    select: &mut Select<usize>,
    ui: &mut Ui,
    input: &mut InputState,
    text: &mut TextSystem,
    event: InputEvent,
) -> Result<()> {
    if let Some(event) = select.event(ui, input, event, &mut Clip)? {
        input.event(ui, event, &mut Clip)?;
    }
    select.dispatch(ui, input)?;
    mount(select, ui, input, text)
}

#[test]
fn twelve_versions_open_by_pointer_then_filter_and_choose_by_keyboard() -> Result<()> {
    let (mut ui, mut input, mut text) = surface()?;
    let options = (0..12)
        .map(|i| SelectOption::new(i, format!("Ara preview {}", i + 1)))
        .collect();
    let mut select = Select::new("engine", "Engine version", options, Some(0));
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    let rect = ui.nodes[ui.find("engine-control").expect("control")].rect;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::PointerDown(point),
    )?;
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::PointerUp(point),
    )?;
    assert!(select.is_open());
    assert_eq!(input.focused(), Some(stable_id("engine-search")));
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::Edit(EditEvent::Insert("preview 12".into())),
    )?;
    assert_eq!(select.query(), "preview 12");
    assert_eq!(select.active().map(|o| o.value), Some(11));
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::Activate,
    )?;
    assert_eq!(select.take_changed(), Some(11));
    assert_eq!(select.selected(), Some(&11));
    assert!(!select.is_open());
    assert_eq!(input.focused(), Some(stable_id("engine-control")));
    Ok(())
}

#[test]
fn hundred_options_reveal_keyboard_target_skip_disabled_and_escape_keeps_value() -> Result<()> {
    let (mut ui, mut input, mut text) = surface()?;
    let mut options: Vec<_> = (0..100)
        .map(|i| SelectOption::new(i, format!("Option {i}")))
        .collect();
    options[99].disabled = Some("Not installed".into());
    let mut select = Select::new("enum", "Render mode", options, Some(1));
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    input.focus(&mut ui, stable_id("enum-control"))?;
    select.key(&mut ui, &mut input, Key::Down)?;
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    select.key(&mut ui, &mut input, Key::End)?;
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    assert_eq!(select.active().map(|o| o.value), Some(98));
    let option = &ui.nodes[ui.find("enum-option-0-98").expect("last enabled option")];
    assert!(
        option.rect.y >= option.clip.y - 0.5
            && option.rect.y + option.rect.h <= option.clip.y + option.clip.h + 0.5,
        "{:?} clipped by {:?}",
        option.rect,
        option.clip
    );
    select.key(&mut ui, &mut input, Key::Home)?;
    assert_eq!(select.active().map(|o| o.value), Some(0));
    select.key(&mut ui, &mut input, Key::Down)?;
    assert_eq!(select.active().map(|o| o.value), Some(1));
    select.key(&mut ui, &mut input, Key::Up)?;
    assert_eq!(select.active().map(|o| o.value), Some(0));
    select.key(&mut ui, &mut input, Key::Escape)?;
    assert_eq!(select.selected(), Some(&1));
    assert_eq!(select.take_changed(), None);
    assert_eq!(input.focused(), Some(stable_id("enum-control")));
    Ok(())
}

#[test]
fn localized_long_options_stay_bounded_and_only_owned_actions_are_consumed() -> Result<()> {
    let (mut ui, mut input, mut text) = surface()?;
    let options = vec![
        SelectOption::new(0, "Rendering quality"),
        SelectOption::new(
            1,
            "إعدادات جودة العرض والتفاصيل والإضاءة مع وصف طويل للاختبار",
        ),
        SelectOption::new(
            2,
            "A long localized option label with detailed information that wraps inside the current narrow surface instead of extending past the window edge",
        ),
    ];
    let mut select = Select::new("quality", "جودة العرض", options, Some(2));
    select.open("");
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    let popup = ui.nodes[ui.find("quality-popover").expect("popup")].rect;
    assert!(
        popup.x >= 0. && popup.w <= 312.5 && popup.x + popup.w <= 320.5,
        "{popup:?}"
    );
    assert!(popup.y >= 0. && popup.y + popup.h <= 300.5, "{popup:?}");
    let long = ui.nodes[ui.find("quality-option-0-2").expect("long option")].rect;
    assert!(long.h > 26. && long.w <= popup.w, "{long:?}");
    ui.actions.push(Action {
        name: "host.save".into(),
        node: stable_id("outside"),
        value: serde_json::Value::Null,
    });
    select.dispatch(&mut ui, &mut input)?;
    assert_eq!(
        ui.drain_actions().next().expect("host action remains").name,
        "host.save"
    );
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::Edit(EditEvent::Insert("不存在".into())),
    )?;
    assert!(select.active().is_none());
    route(
        &mut select,
        &mut ui,
        &mut input,
        &mut text,
        InputEvent::Activate,
    )?;
    assert!(
        select.is_open(),
        "Enter with no matches must not choose an unrelated value"
    );
    Ok(())
}

#[test]
fn reordered_options_reject_stale_clicks_and_preserve_typed_active_value() -> Result<()> {
    let (mut ui, mut input, mut text) = surface()?;
    let mut select = Select::new(
        "mode",
        "Mode",
        vec![
            SelectOption::new(10, "Ten"),
            SelectOption::new(20, "Twenty"),
        ],
        Some(10),
    );
    select.open("");
    mount(&mut select, &mut ui, &mut input, &mut text)?;
    ui.actions.push(Action {
        name: "ara.select.mode.choose".into(),
        node: stable_id("mode-option-0-0"),
        value: serde_json::Value::Null,
    });
    select.set_options(vec![
        SelectOption::new(20, "Twenty"),
        SelectOption::new(10, "Ten"),
    ]);
    select.dispatch(&mut ui, &mut input)?;
    assert_eq!(select.take_changed(), None);
    assert_eq!(select.active().map(|o| o.value), Some(10));
    select.key(&mut ui, &mut input, Key::Enter)?;
    assert_eq!(select.take_changed(), Some(10));
    Ok(())
}

#[test]
fn disappearing_select_anchor_preserves_a_modal_above_it_and_its_capture() -> Result<()> {
    use crate::dialogs::{DialogOptions, Dialogs};
    for removed in [false, true] {
        let (mut ui, mut input, mut text) = surface()?;
        ui.set_stylesheet(&format!("{EDITOR_CSS}{CSS}{}", crate::dialogs::CSS))?;
        let mut select = Select::new(
            "mode",
            "Mode",
            vec![SelectOption::new(1, "One"), SelectOption::new(2, "Two")],
            Some(1),
        );
        select.open("");
        mount(&mut select, &mut ui, &mut input, &mut text)?;
        let mut dialogs = Dialogs::default();
        let mut response = dialogs.confirm(DialogOptions::new(
            "Continue?",
            "The modal remains active while its underlying field changes.",
        ));
        let markup = |control: &str, select: &Select<usize>, dialogs: &Dialogs| {
            format!(
                "<ui id='root'><button id='outside' text='Outside' />{control}{}{}</ui>",
                select.popover_markup(),
                dialogs.markup([320., 300.])
            )
        };
        ui.reconcile(&markup(&select.control_markup(), &select, &dialogs))?;
        ui.frame(&mut text)?;
        select.sync(&mut ui, &mut input)?;
        dialogs.sync(&mut ui, &mut input)?;
        ui.frame(&mut text)?;
        let accept = stable_id("ara-dialog-1-accept");
        let rect = ui.nodes[ui.by_id(accept).expect("modal action")].rect;
        input.event(
            &mut ui,
            InputEvent::PointerDown([rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]),
            &mut Clip,
        )?;
        assert_eq!(input.captured(), Some(accept));
        if removed {
            ui.reconcile(&markup("", &select, &dialogs))?;
            ui.frame(&mut text)?;
        } else {
            let anchor = ui.find(select.control_id()).expect("select anchor");
            ui.set_visible(anchor, false)?;
        }
        select.sync(&mut ui, &mut input)?;
        assert!(!select.is_open());
        assert_eq!(
            input.focused(),
            Some(accept),
            "closing an underlying popup must preserve modal focus"
        );
        assert_eq!(
            input.captured(),
            Some(accept),
            "closing an underlying popup must preserve modal capture"
        );
        dialogs.event(&mut ui, &mut input, InputEvent::Cancel, &mut Clip)?;
        assert_eq!(response.try_take(), Some(false));
        assert_eq!(input.focused(), Some(stable_id("outside")));
        assert_eq!(input.captured(), None);
    }
    Ok(())
}
