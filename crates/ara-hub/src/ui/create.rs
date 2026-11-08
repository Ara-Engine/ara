//! New project: the full window. Template tiles on the left, the project's settings on the
//! right. Problems come from the service's `hub.create` dry run, shown under their field.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::TextRole;
use stillkit::el::{Align, El, Justify, col, el, grid, icon, image, menu, row, text};

use super::images::Pic;
use super::{HubApp, shell};
use crate::model::{Intent, Template, text as fmt};

const SETTINGS: f32 = 360.0;

/// Back, then the page's title.
pub(super) fn header(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    row()
        .gap(14.0)
        .align(Align::Center)
        .child(
            button(l, Tone::Ghost, Size::Md, Some(icons::ARROW_LEFT), "Back")
                .key("create-back")
                .tooltip("Back (Esc)")
                .on_press(|s: &mut HubApp| s.intent(Intent::Back)),
        )
        .child(vdivider(l, 20.0))
        .child(
            page_header(
                l,
                "New project",
                Some("Start from a template or one of the real-art samples."),
                Vec::new(),
            )
            .grow(1.0),
        )
}

pub(super) fn body(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let hub = &s.hub;
    let cols = super::projects::columns(s, 230.0).saturating_sub(1).max(1);
    let mut templates = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(12.0)
        .child(label(l, "Templates"));
    if hub.templates.is_empty() && hub.ready() {
        templates = templates.child(
            text("The Hub service lists no templates.")
                .text_size(12.0)
                .color(l.c.muted_foreground.0),
        );
    }
    templates = templates.child(
        grid(cols).gap(14.0).children(
            hub.templates
                .iter()
                .map(|t| tile(s, t, hub.create.template.as_deref() == Some(t.id.as_str()))),
        ),
    );
    row()
        .gap(24.0)
        .align(Align::Start)
        .child(templates)
        .child(settings(s))
}

/// A template tile: its picture (or a generated cover), name and description. Choosing it
/// selects it here, or opens New project with it from elsewhere.
pub(super) fn tile(s: &HubApp, template: &Template, on: bool) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let file = template
        .thumbnail
        .clone()
        .filter(|p| !s.images.failed(&Pic::File(p.clone())));
    let pic = file
        .map(Pic::File)
        .unwrap_or(Pic::Cover(ara_kit::art::seed(&template.id), l.dark));
    let mut picture = el().w_full().h(120.0).radius(l.radius.md).fill(c.muted.0);
    if let Some(img) = s.images.get(&pic) {
        picture = picture.child(image(img).w_full().h_full().radius(l.radius.md));
    }
    if template.id == "blank" && template.thumbnail.is_none() {
        picture = picture.child(
            el().absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .align(Align::Center)
                .justify(Justify::Center)
                .child(
                    icon(icons::FILE_PLUS)
                        .size(28.0)
                        .color(stillkit::Rgba::new(1.0, 1.0, 1.0, 0.8)),
                ),
        );
    }
    if template.setup.is_some() {
        picture = picture.child(row().absolute(8.0, 8.0).child(chip(
            l,
            Status::Warning,
            "Needs art download",
            false,
        )));
    }
    let mut title = row().gap(6.0).align(Align::Center).child(
        text(template.name.clone())
            .font(TextRole::Headline)
            .color(c.foreground.0)
            .grow(1.0),
    );
    if on {
        title = title.child(
            icon(icons::CIRCLE_CHECK)
                .size(16.0)
                .color(c.info_foreground.0),
        );
    }
    let id = template.id.clone();
    let mut t = hover_card(l)
        .key(format!("template-{}", template.id))
        .pad(6.0)
        .gap(10.0)
        .label(template.name.clone())
        .selected(on)
        .on_press(move |s: &mut HubApp| s.intent(Intent::Template(id.clone())))
        .child(picture)
        .child(
            col()
                .pad_x(6.0)
                .pad_y(0.0)
                .gap(4.0)
                .child(title)
                .child(
                    text(description(template))
                        .text_size(11.5)
                        .wrap(2)
                        .color(c.muted_foreground.0),
                )
                .child(el().h(6.0)),
        );
    if on {
        t = t.border(1.5, c.ring.0);
    }
    t
}

/// The template's own description, else a short one for the built-in samples.
pub(super) fn description(template: &Template) -> String {
    if let Some(description) = &template.description {
        return description.clone();
    }
    match template.id.as_str() {
        "outpost" => "A derelict compound at dusk on 384 m of terrain, plus a night hangar.",
        "showcase" => "A turntable stage: animated characters, hero props and material balls.",
        "starter" => "The `ara init` starter project.",
        "blank" => "An empty scene with a camera and a light.",
        _ => "A project template.",
    }
    .into()
}

/// The first problem the dry run reported for `field`, as a form-row note.
fn problem(s: &HubApp, field: &str) -> Option<String> {
    s.hub
        .create
        .problems_for(field)
        .next()
        .map(|d| d.message.clone())
}

fn settings(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let form = &hub.create;
    let mut panel = card(l).w(SETTINGS).pad(18.0).gap(16.0).child(
        text("Project settings")
            .font(TextRole::Title3)
            .color(c.foreground.0),
    );
    if let Some(template) = form
        .template
        .as_ref()
        .and_then(|id| hub.templates.iter().find(|t| &t.id == id))
    {
        let mut row_el = col()
            .gap(6.0)
            .child(
                text("Template")
                    .text_size(12.0)
                    .weight(560)
                    .color(c.foreground.0),
            )
            .child(
                text(template.name.clone())
                    .text_size(12.5)
                    .color(c.muted_foreground.0),
            );
        if let Some(p) = problem(s, "template") {
            row_el = row_el.child(
                text(p)
                    .text_size(11.0)
                    .wrap(3)
                    .color(c.destructive_foreground.0),
            );
        }
        if let Some(setup) = &template.setup {
            row_el = row_el
                .child(
                    text(setup.message.clone())
                        .text_size(11.5)
                        .wrap(3)
                        .color(c.muted_foreground.0),
                )
                .child(shell::code(s, "copy-setup", &setup.command));
        }
        panel = panel.child(row_el);
    }
    let name_problem = problem(s, "name");
    panel = panel.child(form_row(
        l,
        "Project name",
        field(
            l,
            Size::Md,
            "create-name",
            None,
            "My game",
            &form.name,
            |f| {
                f.on_input(|s: &mut HubApp, t| s.intent(Intent::CreateName(t.into())))
                    .on_submit(|s: &mut HubApp, _| {
                        if s.hub.create.can_submit() {
                            s.intent(Intent::CreateSubmit)
                        }
                    })
            },
        ),
        name_problem.as_deref().map(|p| (p, true)),
    ));
    let path_problem = problem(s, "path");
    let preview = form
        .path()
        .map(|p| format!("Creates {}", fmt::middle(&fmt::path(&p), 52)));
    let location = row()
        .gap(6.0)
        .child(
            field(
                l,
                Size::Md,
                "create-destination",
                Some(icons::FOLDER),
                "Location",
                &form.destination,
                |f| f.on_input(|s: &mut HubApp, t| s.intent(Intent::CreateDestination(t.into()))),
            )
            .grow(1.0),
        )
        .child(
            button(l, Tone::Secondary, Size::Md, None, "Browse…")
                .key("browse-destination")
                .on_press(|s: &mut HubApp| s.intent(Intent::Browse)),
        );
    let note = path_problem
        .as_deref()
        .map(|p| (p, true))
        .or(preview.as_deref().map(|p| (p, false)));
    panel = panel.child(form_row(l, "Location", location, note));
    let engine = if hub.engines.is_empty() {
        text("No engines installed")
            .text_size(12.0)
            .color(c.muted_foreground.0)
    } else {
        engine_select(s)
    };
    let engine_problem = problem(s, "engineVersion");
    panel = panel.child(form_row(
        l,
        "Engine version",
        engine,
        engine_problem.as_deref().map(|p| (p, true)),
    ));
    for diagnostic in form.general() {
        panel = panel.child(banner(
            l,
            Status::Danger,
            icons::CIRCLE_ALERT,
            &diagnostic.message,
            Vec::new(),
        ));
    }
    let label = if form.submitting {
        "Creating…"
    } else {
        "Create project"
    };
    panel.child(divider(l)).child(
        row()
            .gap(8.0)
            .justify(Justify::End)
            .child(
                button(l, Tone::Secondary, Size::Md, None, "Cancel")
                    .key("create-cancel")
                    .on_press(|s: &mut HubApp| s.intent(Intent::Back)),
            )
            .child(enabled(
                button(l, Tone::Primary, Size::Md, Some(icons::PLUS), label)
                    .key("create-submit")
                    .on_press(|s: &mut HubApp| s.intent(Intent::CreateSubmit)),
                form.can_submit(),
            )),
    )
}

/// The engine version dropdown: the trigger, and its menu while open.
fn engine_select(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let hub = &s.hub;
    let current = hub
        .create
        .engine
        .clone()
        .unwrap_or_else(|| "Choose a version".into());
    let open = s.select.as_deref() == Some("create-engine");
    let mut trigger = select(l, Size::Md, "create-engine", &current)
        .w_full()
        .expanded(open)
        .on_press(|s: &mut HubApp| s.toggle_select("create-engine"));
    if open {
        let items = hub
            .engines
            .iter()
            .map(|e| {
                let v = e.version.clone();
                menu_check(
                    l,
                    &format!("engine-{}", e.version),
                    &e.version,
                    hub.create.engine.as_deref() == Some(e.version.as_str()),
                )
                .on_press(move |s: &mut HubApp| {
                    s.select = None;
                    s.intent(Intent::CreateEngine(v.clone()));
                })
            })
            .collect();
        trigger = trigger.overlay(
            menu(menu_panel(l, items))
                .match_width()
                .on_dismiss(|s: &mut HubApp| s.select = None),
        );
    }
    trigger
}
