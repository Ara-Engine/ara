//! Learn: the samples as starting points, the docs present on this machine and a
//! short CLI reference. Deliberately light.
use super::{Ctx, Page, columns, create::description, tracks};
use crate::model::{Hub, Intent};

/// Sidebar and content padding.
const CHROME: f32 = 317.;

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
    if !ctx.docs.is_empty() {
        p.text("section-title", "Docs on this machine");
        p.panel("list", |p| {
            for (row, doc) in ctx.docs.iter().enumerate() {
                if row > 0 {
                    p.leaf("divider");
                }
                p.button(
                    &format!("doc-{row}"),
                    "link-row",
                    &doc.title,
                    Some(Intent::OpenPath(doc.path.clone())),
                    |p| {
                        p.icon("book-open", "accent");
                        p.panel("col fill", |p| {
                            p.text("fg", &doc.title);
                            p.text("tiny muted", &doc.caption);
                        });
                        p.icon("external-link", "icon-sm");
                    },
                );
            }
        });
    }
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
