//! Learn: the samples as starting points, the docs on the open-source repository
//! and a short CLI reference. Deliberately light.
use super::{Ctx, Page, columns, create::description, tracks};
use crate::model::{Hub, Intent};

/// Sidebar and content padding.
const CHROME: f32 = 317.;

/// Docs on GitHub: title, caption and repository path (opened on `main`).
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
        "ara init C:\\Games\\MyGame",
        "Create a project from the command line",
    ),
    (
        "ara hub add C:\\Games\\MyGame",
        "Register an existing project with the Hub",
    ),
    ("ara-editor C:\\Games\\MyGame", "Open the editor directly"),
    ("ara mcp", "Global MCP server for agents (stdio)"),
    ("ara doctor", "Check prerequisites: GPU, .NET, tools"),
];

pub(super) fn body(p: &mut Page, hub: &Hub, ctx: &Ctx) {
    let samples: Vec<_> = hub
        .templates
        .iter()
        .filter(|t| t.id != "blank" && t.thumbnail.is_some())
        .collect();
    if !samples.is_empty() {
        p.text("section-title", "Samples");
        let width = (ctx.size[0] - CHROME).max(260.);
        let cols = columns(width, 300., 20.).min(samples.len().max(2));
        let card = (width - 20. * (cols - 1) as f32) / cols as f32;
        let thumb = ((card - 14.) * 9. / 16.).round().min(260.);
        p.panel_styled(
            "grid",
            &format!("grid-template-columns:{}", tracks(cols)),
            |p| {
                for sample in samples {
                    p.panel("card", |p| {
                        p.panel_styled("thumb", &format!("height:{thumb}px"), |p| {
                            p.thumbnail(
                                ctx,
                                sample.thumbnail.as_deref(),
                                &sample.id,
                                &sample.name,
                                false,
                            );
                        });
                        p.panel("card-body", |p| {
                            p.text("card-name", &sample.name);
                            p.text("template-copy", &description(sample));
                            if let Some(setup) = &sample.setup {
                                p.panel("card-meta", |p| {
                                    p.icon("download", "icon-sm warning");
                                    p.text("warning", "Needs art:");
                                    p.text("mono tiny muted", &setup.command);
                                });
                            }
                        });
                        p.panel("card-actions", |p| {
                            p.action(
                                &format!("learn-create-{}", sample.id),
                                "primary",
                                "plus",
                                "Create from sample",
                                Some(Intent::Template(sample.id.clone())),
                            );
                        });
                    });
                }
            },
        );
    }
    p.text("section-title", "Docs on GitHub");
    p.panel("list", |p| {
        for (row, (title, caption, path)) in DOCS.iter().enumerate() {
            if row > 0 {
                p.leaf("divider");
            }
            let url = format!(
                "https://github.com/{}/blob/main/{path}",
                ara_update::DEFAULT_REPOSITORY
            );
            p.button(
                &format!("doc-{row}"),
                "link-row",
                title,
                Some(Intent::OpenUrl(url)),
                |p| {
                    p.icon("book-open", "accent");
                    p.panel("col fill", |p| {
                        p.text("fg", title);
                        p.text("tiny muted", caption);
                    });
                    p.icon("external-link", "icon-sm");
                },
            );
        }
    });
    p.text("section-title", "Command line");
    p.panel("code-block", |p| {
        for (index, (command, what)) in CLI.iter().enumerate() {
            p.panel("row", |p| {
                p.text("code-text", command);
                p.leaf("fill");
                p.text("tiny muted", what);
                p.copy(
                    hub.copied.as_deref(),
                    &format!("copy-cli-{index}"),
                    command,
                    true,
                );
            });
        }
    });
}
