//! Projects: search/sort/layout toolbar, cards or rows, and the overflow menu.
use super::{Ctx, Page, columns, tracks};
use crate::model::{Hub, Intent, Layout, Project, Sort, State, View, text};

/// Sidebar 236 + edge 1 + content padding 2×40.
const CHROME: f32 = 317.;

pub(super) fn toolbar(p: &mut Page, hub: &Hub, _ctx: &Ctx) {
    if hub.projects.is_empty() {
        return;
    }
    p.panel("toolbar", |p| {
        p.panel_styled("search", "", |p| {
            p.field("search", "search-input", &hub.search, Intent::Search);
            p.icon("search", "search-icon");
            if hub.search.is_empty() {
                p.text("placeholder", "Search projects");
            }
        });
        p.leaf("fill");
        p.text("small muted", "Sort");
        p.panel("segment", |p| {
            for (sort, label) in [(Sort::Recent, "Last opened"), (Sort::Name, "Name")] {
                let class = if hub.prefs.sort == sort {
                    "seg seg-on"
                } else {
                    "seg"
                };
                p.action(
                    &format!("sort-{label}").to_lowercase().replace(' ', "-"),
                    class,
                    "",
                    label,
                    Some(Intent::Sort(sort)),
                );
            }
        });
        p.panel("segment", |p| {
            for (layout, icon, label) in [
                (Layout::Grid, "layout-grid", "Cards"),
                (Layout::List, "list", "List"),
            ] {
                let class = if hub.prefs.layout == layout {
                    "seg seg-icon seg-on"
                } else {
                    "seg seg-icon"
                };
                p.button(
                    &format!("layout-{label}").to_lowercase(),
                    class,
                    label,
                    Some(Intent::Layout(layout)),
                    |p| p.icon(icon, "inherit"),
                );
            }
        });
    });
}

pub(super) fn body(p: &mut Page, hub: &Hub, ctx: &Ctx) {
    if hub.projects.is_empty() {
        if hub.ready() {
            empty(p, hub, ctx);
        }
        return;
    }
    let visible = hub.visible();
    if visible.is_empty() {
        p.panel("empty", |p| {
            p.panel("empty-icon", |p| p.icon("search", "icon-lg"));
            p.text(
                "empty-title",
                &format!("No projects match “{}”", hub.search.trim()),
            );
            p.text("empty-copy", "Search looks at project names and folders.");
        });
        return;
    }
    match hub.prefs.layout {
        Layout::Grid => grid(p, hub, ctx, &visible),
        Layout::List => list(p, hub, ctx, &visible),
    }
}

/// First run: the call to action, then the templates themselves.
fn empty(p: &mut Page, hub: &Hub, ctx: &Ctx) {
    p.panel("empty", |p| {
        p.panel("empty-icon", |p| p.icon("folder-plus", "icon-lg accent"));
        p.text("empty-title", "No projects yet");
        p.text(
            "empty-copy",
            "Create one from a template or a sample, or add a project folder you already have. You can also drop a project folder onto this window.",
        );
        p.panel("row", |p| {
            p.action(
                "empty-new",
                "primary",
                "plus",
                "New project",
                Some(Intent::Navigate(View::Create)),
            );
            p.action(
                "empty-add",
                "",
                "folder-open",
                "Add existing",
                Some(Intent::AddExisting),
            );
        });
    });
    if hub.templates.is_empty() {
        return;
    }
    p.panel("section-head", |p| {
        p.text("section-title", "Start from a template");
        p.text(
            "section-copy",
            "Outpost and Showcase are real-art samples: open one to see what Ara renders today.",
        );
    });
    let width = (ctx.size[0] - CHROME).max(260.);
    let cols = columns(width, 230., 20.).min(hub.templates.len().max(1));
    let tile = (width - 20. * (cols - 1) as f32) / cols as f32;
    let thumb = ((tile - 14.) * 9. / 16.).round().min(240.);
    p.panel_styled(
        "grid",
        &format!("grid-template-columns:{}", tracks(cols)),
        |p| {
            for template in &hub.templates {
                super::create::tile_for(p, ctx, template, thumb, false);
            }
        },
    );
}

/// The badge for a project's state, or its in-flight work.
fn badge(p: &mut Page, hub: &Hub, project: &Project) {
    if let Some(busy) = hub.busy.get(&project.id) {
        p.panel("pill pill-busy", |p| {
            p.leaf("pill-dot pill-dot-busy");
            p.text("", busy.label());
        });
        return;
    }
    match hub.state(&project.id) {
        State::Editor => p.panel("pill pill-open", |p| {
            p.leaf("pill-dot");
            p.text("", "Open in editor");
        }),
        State::Headless => p.panel("pill pill-headless", |p| {
            p.leaf("pill-dot pill-dot-headless");
            p.text("", "Running headless");
        }),
        State::Closed => {}
    }
}

/// Primary and overflow actions, shared by cards and rows.
fn actions(p: &mut Page, hub: &Hub, project: &Project, compact: bool) {
    let id = &project.id;
    let busy = hub.busy.contains_key(id);
    let size = if compact { " compact" } else { "" };
    let enabled = |intent: Intent| (!busy).then_some(intent);
    let missing = !hub
        .engines
        .iter()
        .any(|e| e.version == project.engine_version);
    let open_label = if missing {
        format!("Install {} and open", project.engine_version)
    } else {
        "Open".into()
    };
    match hub.state(id) {
        State::Closed => p.action(
            &format!("open-{id}"),
            &format!("primary{size}"),
            "play",
            &open_label,
            enabled(if missing {
                Intent::InstallAndOpen {
                    project: id.clone(),
                    headless: false,
                }
            } else {
                Intent::Open(id.clone())
            }),
        ),
        State::Headless => p.action(
            &format!("open-{id}"),
            &format!("primary{size}"),
            "monitor",
            "Open editor",
            enabled(Intent::Open(id.clone())),
        ),
        State::Editor => p.action(
            &format!("close-{id}"),
            size.trim(),
            "square",
            "Close",
            enabled(Intent::Close(id.clone())),
        ),
    }
    if !compact && hub.state(id) == State::Closed && !missing {
        p.action(
            &format!("headless-{id}"),
            "",
            "server",
            "Headless",
            enabled(Intent::OpenHeadless(id.clone())),
        );
    }
    if hub.state(id) == State::Headless && !compact {
        p.action(
            &format!("close-{id}"),
            "",
            "square",
            "Close",
            enabled(Intent::Close(id.clone())),
        );
    }
}

/// The overflow-menu button; the menu anchors to it.
fn more(p: &mut Page, project: &Project) {
    p.button(
        &format!("more-{}", project.id),
        "ghost icon-compact",
        "More actions",
        Some(Intent::Menu(Some(project.id.clone()))),
        |p| p.icon("ellipsis", "inherit"),
    );
}

fn name(p: &mut Page, hub: &Hub, project: &Project, class: &str, chars: usize) {
    match &hub.rename {
        Some((id, draft)) if *id == project.id => {
            p.field(
                "rename-field",
                &format!("rename {class}"),
                draft,
                Intent::RenameDraft,
            );
        }
        _ => p.text(class, &text::clip(&project.name, chars)),
    }
}

fn grid(p: &mut Page, hub: &Hub, ctx: &Ctx, visible: &[&Project]) {
    let width = (ctx.size[0] - CHROME).max(260.);
    let cols = columns(width, 250., 20.);
    let card = (width - 20. * (cols - 1) as f32) / cols as f32;
    let thumb = ((card - 14.) * 9. / 16.).round();
    p.panel_styled(
        "grid",
        &format!("grid-template-columns:{}", tracks(cols)),
        |p| {
            for project in visible {
                let hot = hub.highlight.as_deref() == Some(project.id.as_str());
                let class = if hot { "card card-hot" } else { "card" };
                p.panel_key(&project.id, class, |p| {
                    p.panel_styled("thumb", &format!("height:{thumb}px"), |p| {
                        p.thumbnail(
                            ctx,
                            project.thumbnail.as_deref(),
                            &project.id,
                            &project.name,
                            false,
                        );
                        p.panel("thumb-badge", |p| badge(p, hub, project));
                    });
                    p.panel("card-body", |p| {
                        p.panel("card-title", |p| {
                            name(
                                p,
                                hub,
                                project,
                                "card-name fill",
                                ((card - 60.) / 8.) as usize,
                            );
                            more(p, project);
                        });
                        let chars = ((card - 20.) / 6.6) as usize;
                        p.text(
                            "card-path",
                            &text::middle(&text::path(&project.path), chars),
                        );
                        p.panel("card-meta", |p| {
                            p.icon("clock", "icon-sm");
                            p.text("", &text::relative(project.last_opened, ctx.now_ms));
                            p.leaf("fill");
                            engine_pill(p, hub, project);
                        });
                    });
                    p.panel("card-actions", |p| actions(p, hub, project, false));
                });
            }
        },
    );
}

fn engine_pill(p: &mut Page, hub: &Hub, project: &Project) {
    let installed = hub
        .engines
        .iter()
        .any(|e| e.version == project.engine_version);
    if installed || hub.engines.is_empty() {
        p.panel("pill", |p| p.text("", &project.engine_version));
    } else {
        p.panel("pill pill-warn", |p| {
            p.icon("triangle-alert", "icon-sm inherit");
            p.text("", &format!("{} missing", project.engine_version));
        });
    }
}

fn list(p: &mut Page, hub: &Hub, ctx: &Ctx, visible: &[&Project]) {
    let width = (ctx.size[0] - CHROME).max(600.);
    p.panel("list", |p| {
        p.panel("list-head", |p| {
            p.panel_styled("", "width:72px", |_| {});
            p.text("fill", "NAME");
            p.panel("col-state", |p| p.text("", "STATUS"));
            p.text("col-time", "LAST OPENED");
            p.panel("col-engine", |p| p.text("", "ENGINE"));
            p.panel("col-actions", |_| {});
        });
        for (row, project) in visible.iter().enumerate() {
            if row > 0 {
                p.leaf("divider");
            }
            let hot = hub.highlight.as_deref() == Some(project.id.as_str());
            let class = if hot { "list-row list-hot" } else { "list-row" };
            p.panel_key(&project.id, class, |p| {
                p.panel("list-thumb", |p| {
                    p.thumbnail(
                        ctx,
                        project.thumbnail.as_deref(),
                        &project.id,
                        &project.name,
                        true,
                    );
                });
                p.panel("col-name", |p| {
                    name(p, hub, project, "fg", 60);
                    let chars = ((width - 640.) / 6.6).max(24.) as usize;
                    p.text(
                        "tiny muted",
                        &text::middle(&text::path(&project.path), chars),
                    );
                });
                p.panel("col-state", |p| {
                    badge(p, hub, project);
                    if hub.state(&project.id) == State::Closed
                        && !hub.busy.contains_key(&project.id)
                    {
                        p.text("small muted", "Closed");
                    }
                });
                p.text("col-time", &text::relative(project.last_opened, ctx.now_ms));
                p.panel("col-engine", |p| engine_pill(p, hub, project));
                p.panel("col-actions", |p| {
                    actions(p, hub, project, true);
                    more(p, project);
                });
            });
        }
    });
}

/// The overflow menu of `hub.menu`, anchored under its button.
pub(super) fn menu(p: &mut Page, hub: &Hub, ctx: &Ctx) {
    let (Some(id), Some(anchor)) = (&hub.menu, ctx.anchor) else {
        return;
    };
    let Some(project) = hub.project(id) else {
        return;
    };
    p.button(
        "menu-scrim",
        "scrim",
        "Close menu",
        Some(Intent::Menu(None)),
        |_| {},
    );
    let height = 6. * 33. + 9. + 12.;
    let left = (anchor[0] + anchor[2] - 236.).clamp(8., ctx.size[0] - 244.);
    let below = anchor[1] + anchor[3] + 4.;
    let top = if below + height > ctx.size[1] - 8. {
        (anchor[1] - height - 4.).max(8.)
    } else {
        below
    };
    let state = hub.state(id);
    let busy = hub.busy.contains_key(id);
    let missing = !hub
        .engines
        .iter()
        .any(|e| e.version == project.engine_version);
    let when = |ok: bool, intent: Intent| (ok && !busy).then_some(intent);
    p.panel_styled("menu", &format!("left:{left}px; top:{top}px"), |p| {
        p.action(
            "menu-open",
            "menu-item",
            "play",
            if missing {
                "Install and open in editor"
            } else {
                "Open in editor"
            },
            when(
                state != State::Editor,
                if missing {
                    Intent::InstallAndOpen {
                        project: id.clone(),
                        headless: false,
                    }
                } else {
                    Intent::Open(id.clone())
                },
            ),
        );
        p.action(
            "menu-headless",
            "menu-item",
            "server",
            if missing {
                "Install and open headless"
            } else {
                "Open headless"
            },
            when(
                state == State::Closed,
                if missing {
                    Intent::InstallAndOpen {
                        project: id.clone(),
                        headless: true,
                    }
                } else {
                    Intent::OpenHeadless(id.clone())
                },
            ),
        );
        p.action(
            "menu-close",
            "menu-item",
            "square",
            "Close",
            when(state != State::Closed, Intent::Close(id.clone())),
        );
        p.panel("menu-sep", |p| p.leaf("menu-line"));
        p.action(
            "menu-rename",
            "menu-item",
            "pencil",
            "Rename",
            when(true, Intent::Rename(id.clone())),
        );
        p.action(
            "menu-reveal",
            "menu-item",
            "folder-open",
            "Show in Explorer",
            Some(Intent::Reveal(id.clone())),
        );
        p.action(
            "menu-remove",
            "menu-item menu-danger",
            "trash",
            "Remove from list",
            when(true, Intent::Remove(project.id.clone())),
        );
    });
}
