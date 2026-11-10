//! Learn: the samples as starting points, the docs on GitHub and a short CLI
//! reference. Deliberately light.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, col, el, grid, icon, row, text};

use super::{HubApp, create, shell};
use crate::model::Intent;

/// Docs on GitHub: title, caption and repository path.
const DOCS: &[(&str, &str, &str)] = &[
    (
        "Read me first",
        "Requirements, installation and first commands",
        "release/README-FIRST.txt",
    ),
    ("Ara README", "What Ara is and how to build it", "README.md"),
    (
        "Samples guide",
        "Outpost and Showcase: fetching art, building and capturing",
        "samples/README.md",
    ),
    (
        "Hub and global MCP",
        "Projects, lifecycle and agent setup reference",
        "crates/ara-hub-service/AGENTS.md",
    ),
    (
        "Known issues",
        "What does not work yet in this alpha",
        "docs/KNOWN_ISSUES.md",
    ),
    (
        "Engine decisions",
        "The locked design decisions behind Ara",
        "docs/decisions.md",
    ),
];

const CLI: &[(&str, &str)] = &[
    (
        "ara init ~/Games/MyGame",
        "Create a project from the command line",
    ),
    (
        "ara hub add ~/Games/MyGame",
        "Register an existing project with the Hub",
    ),
    ("ara-editor ~/Games/MyGame", "Open the editor directly"),
    ("ara mcp", "The global MCP server for agents (stdio)"),
    ("ara doctor", "Check prerequisites: GPU, .NET, tools"),
];

pub(super) fn body(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let mut page = col().gap(24.0);
    let samples: Vec<_> = s
        .hub
        .templates
        .iter()
        .filter(|t| t.id != "blank" && t.thumbnail.is_some())
        .collect();
    if !samples.is_empty() {
        page = page.child(
            col()
                .gap(12.0)
                .child(section(
                    l,
                    "Samples",
                    Some("Real-art projects to explore and build from."),
                    Vec::new(),
                ))
                .child(
                    grid(super::projects::columns(s, 280.0).min(samples.len().max(2) as u16))
                        .gap(16.0)
                        .children(samples.into_iter().map(|t| create::tile(s, t, false))),
                ),
        );
    }
    // The docs live on the open-source repository (opened on `main`).
    let mut list = card(l);
    for (i, (title, caption, path)) in DOCS.iter().enumerate() {
        if i > 0 {
            list = list.child(divider(l));
        }
        let url = format!(
            "https://github.com/{}/blob/main/{path}",
            ara_update::DEFAULT_REPOSITORY
        );
        let hover = c.foreground.0;
        list = list.child(
            row()
                .key(format!("doc-{i}"))
                .h(52.0)
                .pad_x(14.0)
                .gap(12.0)
                .align(Align::Center)
                .label(*title)
                .color(c.muted_foreground.0)
                .cascade_ink()
                .hovered(move |e| e.color(hover))
                .on_press(move |s: &mut HubApp| s.intent(Intent::OpenUrl(url.clone())))
                .child(icon(icons::BOOK_OPEN).size(16.0).color(c.info_foreground.0))
                .child(
                    col()
                        .gap(2.0)
                        .grow(1.0)
                        .child(
                            text(*title)
                                .text_size(12.5)
                                .weight(560)
                                .color(c.foreground.0),
                        )
                        .child(text(*caption).text_size(11.5)),
                )
                .child(icon(icons::EXTERNAL_LINK).size(14.0)),
        );
    }
    page = page.child(
        col()
            .gap(12.0)
            .child(section(l, "Docs on GitHub", None, Vec::new()))
            .child(list),
    );
    let mut cli = card(l);
    for (i, (command, what)) in CLI.iter().enumerate() {
        if i > 0 {
            cli = cli.child(divider(l));
        }
        cli = cli.child(
            row()
                .h(46.0)
                .pad_x(14.0)
                .gap(12.0)
                .align(Align::Center)
                .child(
                    icon(icons::SQUARE_TERMINAL)
                        .size(15.0)
                        .color(c.muted_foreground.0),
                )
                .child(
                    text(*command)
                        .text_size(12.0)
                        .family(mono())
                        .color(c.foreground.0)
                        .w(260.0),
                )
                .child(
                    text(*what)
                        .text_size(12.0)
                        .color(c.muted_foreground.0)
                        .grow(1.0),
                )
                .child(shell::copy(s, &format!("copy-cli-{i}"), command)),
        );
    }
    page.child(
        col()
            .gap(12.0)
            .child(section(l, "Command line", None, Vec::new()))
            .child(cli),
    )
    .child(el().h(8.0))
}
