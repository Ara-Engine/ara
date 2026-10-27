//! Create: template tiles on the left, name/location/engine on the right.
//! Problems come from the service's `hub.create` dry run, shown under their field.
use super::{Ctx, Page, columns, shell, tracks};
use crate::model::{Hub, Intent, Pick, Template, text};

/// Sidebar, padding, settings column and gap.
const CHROME: f32 = 317. + 380. + 28.;

pub(super) fn body(p: &mut Page, hub: &Hub, ctx: &Ctx) {
    p.panel("create", |p| {
        p.panel("templates", |p| {
            p.text("section-title", "Templates");
            let width = (ctx.size[0] - CHROME).max(220.);
            let cols = columns(width, 210., 14.);
            let tile = (width - 14. * (cols - 1) as f32) / cols as f32;
            let thumb = ((tile - 14.) * 9. / 16.).round();
            p.panel_styled(
                "grid",
                &format!("grid-template-columns:{}; gap:14px", tracks(cols)),
                |p| {
                    for template in &hub.templates {
                        let on = hub.create.template.as_deref() == Some(template.id.as_str());
                        tile_for(p, ctx, template, thumb, on);
                    }
                },
            );
            if hub.templates.is_empty() && hub.ready() {
                p.text("section-copy", "The Hub service lists no templates.");
            }
        });
        settings(p, hub);
    });
}

/// A template tile; choosing it opens Create with that template (`on` = selected).
pub(super) fn tile_for(p: &mut Page, ctx: &Ctx, template: &Template, thumb: f32, on: bool) {
    let class = if on {
        "template template-on"
    } else {
        "template"
    };
    p.button(
        &format!("template-{}", template.id),
        class,
        &template.name,
        Some(Intent::Template(template.id.clone())),
        |p| {
            p.panel_styled("thumb", &format!("height:{thumb}px"), |p| {
                if template.id == "blank" && template.thumbnail.is_none() {
                    p.panel("tile tile-1", |p| p.icon("file-plus", "icon-xl inherit"));
                } else if template.id == "starter"
                    && template
                        .thumbnail
                        .as_deref()
                        .and_then(|path| (ctx.thumbnail)(path))
                        .is_none()
                {
                    if let Some(path) = &template.thumbnail {
                        p.wanted.push(path.clone());
                    }
                    p.image("template-starter", "thumb-image");
                } else {
                    p.thumbnail(
                        ctx,
                        template.thumbnail.as_deref(),
                        &template.id,
                        &template.name,
                        false,
                    );
                }
                if template.setup.is_some() {
                    p.panel("thumb-badge", |p| {
                        p.panel("pill pill-warn", |p| {
                            p.icon("download", "icon-sm inherit");
                            p.text("", "Needs art download");
                        });
                    });
                }
            });
            p.panel("template-body", |p| {
                p.panel("row", |p| {
                    p.text("template-name fill", &template.name);
                    if on {
                        p.icon("circle-check", "accent");
                    }
                });
                p.text("template-copy", &description(template));
            });
        },
    );
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
        _ => "A project template.",
    }
    .into()
}

fn problems(p: &mut Page, hub: &Hub, field: &str) {
    for diagnostic in hub.create.problems_for(field) {
        p.panel("problem", |p| {
            p.icon("circle-alert", "icon-sm destructive");
            p.text("problem-copy", &diagnostic.message);
        });
    }
}

fn settings(p: &mut Page, hub: &Hub) {
    let form = &hub.create;
    p.panel("settings", |p| {
        p.text("section-title", "Project settings");
        let template = form
            .template
            .as_ref()
            .and_then(|id| hub.templates.iter().find(|t| &t.id == id));
        if let Some(template) = template {
            p.panel("form-row", |p| {
                p.text("label", "Template");
                p.text("fg", &template.name);
                problems(p, hub, "template");
                if let Some(setup) = &template.setup {
                    p.panel("code", |p| {
                        p.text("code-text fill", &setup.command);
                        p.copy(hub.copied.as_deref(), "copy-setup", &setup.command, true);
                    });
                }
            });
        }
        p.panel("form-row", |p| {
            p.text("label", "Project name");
            let class = if form.problems_for("name").next().is_some() {
                "field-error"
            } else {
                ""
            };
            p.field("create-name", class, &form.name, Intent::CreateName);
            problems(p, hub, "name");
        });
        p.panel("form-row", |p| {
            p.text("label", "Location");
            p.panel("row", |p| {
                let class = if form.problems_for("path").next().is_some() {
                    "fill field-error"
                } else {
                    "fill"
                };
                p.field(
                    "create-destination",
                    class,
                    &form.destination,
                    Intent::CreateDestination,
                );
                shell::browse(p, "browse-destination", Pick::Destination);
            });
            if let Some(path) = form.path() {
                p.text(
                    "preview-path",
                    &format!("Creates {}", text::middle(&text::path(&path), 60)),
                );
            }
            problems(p, hub, "path");
        });
        p.panel("form-row", |p| {
            p.text("label", "Engine version");
            if hub.engines.is_empty() {
                p.text("small muted", "No engines installed");
            } else {
                p.dropdown(
                    "create-engine",
                    "Engine version",
                    hub.engines
                        .iter()
                        .map(|engine| {
                            (
                                engine.version.clone(),
                                Intent::CreateEngine(engine.version.clone()),
                            )
                        })
                        .collect(),
                    form.engine
                        .as_ref()
                        .map(|version| Intent::CreateEngine(version.clone())),
                );
            }
            problems(p, hub, "engineVersion");
        });
        for diagnostic in form.general() {
            p.panel("problem", |p| {
                p.icon("circle-alert", "icon-sm destructive");
                p.text("problem-copy", &diagnostic.message);
            });
        }
        p.leaf("divider");
        p.panel("row", |p| {
            p.leaf("fill");
            p.action(
                "create-cancel",
                "",
                "",
                "Cancel",
                Some(Intent::Navigate(crate::model::View::Projects)),
            );
            let label = if form.submitting {
                "Creating…"
            } else {
                "Create project"
            };
            p.action(
                "create-submit",
                "primary",
                "plus",
                label,
                form.can_submit().then_some(Intent::CreateSubmit),
            );
        });
    });
}
