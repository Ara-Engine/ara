use crate::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// All native widgets, component projection, four editor panels and three game screens.
/// Every story includes Dark/Light, Arabic RTL, pseudo-localization and three input devices.
pub fn catalogue() -> Vec<Story> {
    let mut stories = Vec::new();
    for tag in ara_ui::source::stylesheet::TYPES
        .iter()
        .copied()
        .filter(|tag| !matches!(*tag, "ui" | "template" | "instance" | "slot"))
    {
        let body = match tag {
            "image" => "<image id='sample' test-id='subject' src='@fixtures/checker.png#im_checker' style='width:120px;height:100px;'/>".into(),
            "field" | "input" => format!("<{tag} id='sample' test-id='subject' text='' bind='text' label='loc:search' action='game.edit'/>"),
            "toggle" => "<toggle id='sample' test-id='subject' text='loc:example' bind='checked' action='game.toggle'/>".into(),
            "slider" | "progress" => format!("<{tag} id='sample' test-id='subject' value='0.65' bind='value' label='loc:volume' action='game.volume'/>"),
            "button" => "<button id='sample' test-id='subject' text='loc:resume' action='game.resume'/>".into(),
            "tabs" => "<tabs id='sample' test-id='subject' action='game.tab'><panel id='page1'><text id='page-title' text='loc:example'/></panel><panel id='page2'><text id='page-title2' text='loc:inventory'/></panel></tabs>".into(),
            "dropdown" | "menu" => format!("<{tag} id='sample' test-id='subject' text='loc:example' action='game.open'><button id='choice' text='loc:resume' action='game.choose' style='position:absolute;top:48px;width:100%;'/></{tag}>"),
            "splitter" => "<panel id='split-row' class='split-row'><panel id='left' style='flex-grow:1;'><text id='left-text' text='loc:hierarchy'/></panel><splitter id='sample' test-id='subject' action='editor.split'/><panel id='right' style='flex-grow:1;'><text id='right-text' text='loc:inspector'/></panel></panel>".into(),
            "text" | "tab" => format!("<{tag} id='sample' test-id='subject' text='loc:example'/>"),
            // A popover is placed beside its anchor (a root child, after layout).
            "popover" => "<button id='anchor' text='loc:example' action='game.open'/><popover id='sample' test-id='subject' class='card' anchor='anchor' placement='bottom-start'><text id='example' text='loc:inventory'/><button id='child-action' text='loc:resume' action='game.resume'/></popover>".into(),
            "tooltip" => "<button id='anchor' text='loc:resume' action='game.resume'/><tooltip id='sample' test-id='subject' class='card' anchor='anchor' placement='bottom-start'><text id='example' text='loc:example'/></tooltip>".into(),
            "feedback" => "<feedback id='sample' test-id='subject' class='card' style='left:24px;top:64px;font-size:13px'><text id='example' text='loc:dragFeedback'/></feedback>".into(),
            "banner" => "<banner id='sample' test-id='subject' class='card' style='font-size:13px'><text id='example' text='loc:importResult'/></banner>".into(),
            _ => format!("<{tag} id='sample' test-id='subject'><text id='example' text='loc:example'/><button id='child-action' text='loc:resume' action='game.resume'/></{tag}>"),
        };
        let mut story = make(&format!("widget-{tag}"), tag, &body);
        if tag == "image" {
            story.images.push(StoryImage {
                source: "@fixtures/checker.png#im_checker".into(),
                width: 2,
                height: 2,
                rgba: vec![
                    52, 107, 241, 255, 27, 78, 216, 255, 27, 78, 216, 255, 52, 107, 241, 255,
                ],
            });
        }
        match tag {
            "button" => story.steps.extend([
                step(StepKind::Activate, json!(null)),
                step(StepKind::Focused, json!(null)),
                step(StepKind::Action, json!("game.resume")),
            ]),
            "toggle" => story.steps.extend([
                step(StepKind::Activate, json!(null)),
                step(StepKind::Value, json!(true)),
                step(StepKind::Action, json!("game.toggle")),
            ]),
            "input" | "field" => story.steps.extend([
                step(StepKind::Focus, json!(null)),
                step(StepKind::Text, json!("!")),
                step(StepKind::Value, json!("Outpost!")),
                step(StepKind::Action, json!("game.edit")),
            ]),
            "slider" | "progress" => story.steps.push(step(StepKind::Value, json!(0.65))),
            "dropdown" | "menu" => {
                story.steps.push(step(StepKind::Activate, json!(null)));
                story.steps.push(StoryStep {
                    kind: StepKind::Visible,
                    query: id_query("choice"),
                    value: Value::Null,
                });
            }
            _ => {}
        }
        stories.push(story);
    }
    stories.push(make("component-slots", "@ui/card.ui#uc_card", "<template id='card'><panel id='card-root' test-id='subject' class='card'><slot id='content-slot' name='content'><text id='default' text='loc:example'/></slot></panel></template><instance id='sample' template='card'><button id='projected' slot='content' text='loc:resume' action='game.resume'/></instance>"));
    for (name, body) in [
        (
            "inspector",
            "<panel id='sample' test-id='subject' class='card'><text id='panel-title' class='heading' text='loc:inspector'/><panel id='name-row' class='row'><text id='name-label' text='loc:name'/><field id='name' text='' bind='text' label='loc:name' action='editor.name'/></panel><panel id='value-row' class='row'><text id='value-label' text='loc:volume'/><slider id='volume' bind='value'/></panel><toggle id='enabled' text='loc:enabled' bind='checked'/></panel>",
        ),
        (
            "dock",
            "<dock id='sample' test-id='subject' class='card'><text id='panel-title' class='heading' text='loc:dock'/><panel id='panes' class='split-row'><panel id='hierarchy-pane' class='card' style='flex-grow:1;'><text id='hierarchy-title' text='loc:hierarchy'/><button id='entity' text='Outpost'/></panel><splitter id='divider'/><panel id='inspector-pane' class='card' style='flex-grow:1;'><text id='inspector-title' text='loc:inspector'/><field id='name' text='' bind='text'/></panel></panel></dock>",
        ),
        (
            "hierarchy",
            "<tree id='sample' test-id='subject' class='card'><text id='panel-title' class='heading' text='loc:hierarchy'/><button id='scene' text='Outpost' action='editor.select'/><panel id='children' style='padding-left:12px;'><button id='player' text='loc:player' action='editor.select'/><button id='camera' text='loc:camera' action='editor.select'/></panel></tree>",
        ),
        (
            "console",
            "<panel id='sample' test-id='subject' class='card'><text id='panel-title' class='heading' text='loc:console'/><field id='filter' text='' label='loc:search' action='editor.filter'/><scroll id='messages' style='height:100px;'><text id='log-ready' text='loc:ready'/><text id='log-warning' class='warning' text='loc:warning'/><text id='log-error' class='error' text='loc:error'/></scroll><button id='clear' text='loc:clear' action='editor.clear'/></panel>",
        ),
        (
            "pause",
            "<modal id='sample' test-id='subject' class='card'><text id='panel-title' class='heading' text='loc:paused'/><button id='resume' text='loc:resume' action='game.resume'/><button id='settings' text='loc:settings' action='game.settings'/><button id='quit' text='loc:quit' action='game.quit'/></modal>",
        ),
        (
            "inventory",
            "<panel id='sample' test-id='subject' class='card'><text id='panel-title' class='heading' text='loc:inventory'/><field id='search' text='' label='loc:search' action='game.search'/><panel id='items' class='row'><button id='scrap' text='loc:scrap' action='game.equip'/><button id='medkit' text='loc:medkit' action='game.equip'/></panel><text id='capacity' class='muted' text='12 / 24'/></panel>",
        ),
        (
            "hud",
            "<panel id='sample' test-id='subject'><panel id='health-row' class='row'><text id='health-label' text='loc:health'/><progress id='health' bind='value' label='loc:health'/></panel><text id='objective' class='heading' text='loc:objective'/><text id='ammo' text='24 / 120'/><text id='interact' class='muted' text='loc:interact'/></panel>",
        ),
    ] {
        let mut story = make(name, &format!("@ui/{name}.ui#uc_root"), body);
        if name == "pause" {
            story.steps.extend([
                StoryStep {
                    kind: StepKind::Activate,
                    query: Query {
                        by: QueryBy::Role,
                        value: "button".into(),
                        name: Some("loc:resume".into()),
                    },
                    value: Value::Null,
                },
                step(StepKind::Action, json!("game.resume")),
            ]);
        }
        stories.push(story);
    }
    stories
}

fn make(name: &str, component: &str, body: &str) -> Story {
    Story {
        name: name.into(), component: component.into(),
        markup: format!("<ui id='root'><text id='story-title' class='heading' text='{name}'/>{body}</ui>"),
        stylesheet: "#sample { min-height:48px; } .split-row { flex-direction:row; gap:6px; height:150px; } splitter { width:8px; background:var(--border); } .warning { color:var(--warning-foreground); } .error { color:var(--destructive-foreground); }".into(),
        fixture_type: "UiStoryFixture".into(),
        fixture: BTreeMap::from([("text".into(), json!("Outpost")), ("value".into(), json!(0.65))]),
        locales: locales(), views: views(),
        transitions: vec![Transition { node: "story-title".into(), from: [-4.0, 0.0], to: [0.0, 0.0], start_ms: 0, duration_ms: 200 }],
        steps: vec![step(StepKind::Visible, Value::Null)], images: Vec::new(),
    }
}
fn id_query(value: &str) -> Query {
    Query {
        by: QueryBy::TestId,
        value: value.into(),
        name: None,
    }
}
fn step(kind: StepKind, value: Value) -> StoryStep {
    StoryStep {
        kind,
        query: id_query("subject"),
        value,
    }
}
fn views() -> Vec<StoryView> {
    [
        ("en", Theme::Dark, InputMode::Mouse, 480, 320, 96, 0),
        ("en", Theme::Light, InputMode::Keyboard, 480, 320, 96, 200),
        ("ar", Theme::Dark, InputMode::Gamepad, 480, 320, 144, 200),
        ("ar", Theme::Light, InputMode::Mouse, 480, 320, 96, 0),
        (
            "qps-ploc",
            Theme::Dark,
            InputMode::Keyboard,
            320,
            360,
            96,
            0,
        ),
        (
            "qps-ploc",
            Theme::Light,
            InputMode::Gamepad,
            320,
            360,
            144,
            200,
        ),
    ]
    .into_iter()
    .map(
        |(locale, theme, input, width, height, dpi, time_ms)| StoryView {
            width,
            height,
            dpi,
            locale: locale.into(),
            input,
            theme,
            time_ms,
        },
    )
    .collect()
}
fn locales() -> Vec<Locale> {
    let pairs = [
        ("example", "Component preview", "معاينة المكوّن"),
        ("resume", "Resume", "استئناف"),
        ("search", "Search", "بحث"),
        ("volume", "Volume", "مستوى الصوت"),
        ("inventory", "Inventory", "المخزون"),
        ("hierarchy", "Hierarchy", "التسلسل"),
        ("inspector", "Inspector", "المفتّش"),
        ("name", "Name", "الاسم"),
        ("enabled", "Enabled", "مفعّل"),
        ("dock", "Workspace", "مساحة العمل"),
        ("player", "Player", "اللاعب"),
        ("camera", "Camera", "الكاميرا"),
        ("console", "Console", "وحدة التحكم"),
        ("ready", "World ready", "العالم جاهز"),
        ("warning", "Missing asset preview", "معاينة الأصل مفقودة"),
        ("error", "Script compile failed", "فشل تجميع البرنامج"),
        ("clear", "Clear", "مسح"),
        ("paused", "Paused", "متوقف مؤقتًا"),
        ("settings", "Settings", "الإعدادات"),
        ("quit", "Quit", "خروج"),
        ("scrap", "Scrap", "خردة"),
        ("medkit", "Medkit", "حقيبة إسعاف"),
        ("health", "Health", "الصحة"),
        ("objective", "Reach the outpost", "الوصول إلى الموقع"),
        ("interact", "Interact", "تفاعل"),
        (
            "dragFeedback",
            "An entity cannot be moved inside itself or its descendants",
            "لا يمكن نقل عنصر إلى نفسه أو إلى أحد العناصر التابعة له",
        ),
        (
            "importResult",
            "Import complete · 2 created · 0 replaced · 0 skipped · 0 renamed",
            "اكتمل الاستيراد · أُنشئ 2 · استُبدل 0 · تم تخطي 0 · أُعيدت تسمية 0",
        ),
    ];
    let en: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, en, _)| ((*k).into(), (*en).into()))
        .collect();
    vec![
        Locale {
            id: "en".into(),
            messages: en.clone(),
            ..Locale::default()
        },
        Locale {
            id: "ar".into(),
            rtl: true,
            messages: pairs
                .iter()
                .map(|(k, _, ar)| ((*k).into(), (*ar).into()))
                .collect(),
            ..Locale::default()
        },
        Locale {
            id: "qps-ploc".into(),
            pseudo: true,
            messages: en,
            ..Locale::default()
        },
    ]
}
