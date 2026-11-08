//! Learn: the samples as starting points, the docs on this machine and a short CLI
//! reference. Deliberately light.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, col, el, grid, icon, row, text};

use super::{HubApp, create, shell};
use crate::model::Intent;

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
    if !s.docs.is_empty() {
        let mut list = card(l);
        for (i, doc) in s.docs.iter().enumerate() {
            if i > 0 {
                list = list.child(divider(l));
            }
            let path = doc.path.clone();
            let hover = c.foreground.0;
            list = list.child(
                row()
                    .key(format!("doc-{i}"))
                    .h(52.0)
                    .pad_x(14.0)
                    .gap(12.0)
                    .align(Align::Center)
                    .label(doc.title.clone())
                    .color(c.muted_foreground.0)
                    .cascade_ink()
                    .hovered(move |e| e.color(hover))
                    .on_press(move |s: &mut HubApp| s.intent(Intent::OpenPath(path.clone())))
                    .child(icon(icons::BOOK_OPEN).size(16.0).color(c.info_foreground.0))
                    .child(
                        col()
                            .gap(2.0)
                            .grow(1.0)
                            .child(
                                text(doc.title.clone())
                                    .text_size(12.5)
                                    .weight(560)
                                    .color(c.foreground.0),
                            )
                            .child(text(doc.caption.clone()).text_size(11.5)),
                    )
                    .child(icon(icons::EXTERNAL_LINK).size(14.0)),
            );
        }
        page = page.child(
            col()
                .gap(12.0)
                .child(section(l, "Docs on this machine", None, Vec::new()))
                .child(list),
        );
    }
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
