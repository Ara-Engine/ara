//! Help › About Ara (version, build, platform, GPU, credits) and System Info (the
//! renderer's device: identity, features, limits), from `crate::about`. Copy puts what
//! the window shows on the clipboard.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, col, el, row, text};

use super::Studio;

/// Which window is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    /// About Ara.
    About,
    /// System Info.
    System,
}

/// A titled group of label/value rows (unavailable values dimmed).
fn rows(s: &Studio, heading: Option<&str>, rows: Vec<(&str, String, bool)>) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let mut out = col().gap(2.0);
    if let Some(heading) = heading {
        out = out.child(
            text(heading.to_owned())
                .text_size(11.5)
                .weight(600)
                .color(c.muted_foreground.0),
        );
    }
    for (key, value, on) in rows {
        out = out.child(
            row()
                .h(22.0)
                .gap(12.0)
                .align(Align::Center)
                .child(
                    text(key.to_owned())
                        .text_size(12.0)
                        .color(c.muted_foreground.0)
                        .w(130.0),
                )
                .child(text(value).text_size(12.0).color(if on {
                    c.foreground.0
                } else {
                    c.muted_foreground.0.with_alpha(0.6)
                })),
        );
    }
    out
}

/// The open window, if any.
pub(super) fn overlay(s: &Studio) -> Option<stillkit::el::Overlay<Studio>> {
    let page = s.local.about?;
    let l = &s.look;
    let c = &l.c;
    let close = |s: &mut Studio| s.local.about = None;
    let (title, body, copy) = match page {
        Page::About => {
            let details = crate::about::about_details(&s.local.device);
            let copy = details
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            let body = col()
                .gap(12.0)
                .child(
                    text("A Rust game engine built by agents, for agents and humans.")
                        .text_size(12.5)
                        .color(c.muted_foreground.0),
                )
                .child(rows(
                    s,
                    None,
                    details.into_iter().map(|(k, v)| (k, v, true)).collect(),
                ))
                .child(
                    text("Inter and JetBrains Mono (OFL), Lucide icons (ISC). See THIRD_PARTY_NOTICES.md.")
                        .text_size(11.0)
                        .wrap(2)
                        .color(c.muted_foreground.0),
                );
            ("About Ara", body, copy)
        }
        Page::System => match crate::about::parse_device(&s.local.device) {
            Some(info) => {
                let mut body = col().gap(12.0);
                for (_, heading, details) in crate::about::system_sections(&info) {
                    body = body.child(rows(s, Some(heading), details));
                }
                // Copy gives the whole device report, beyond what the window shows.
                let report = serde_json::to_string_pretty(&info).unwrap_or_default();
                ("System info", body, report)
            }
            None => (
                "System info",
                col().child(
                    text("The renderer has not started yet, so there is no device to report.")
                        .text_size(12.0)
                        .color(c.muted_foreground.0),
                ),
                String::new(),
            ),
        },
    };
    let mut buttons = Vec::new();
    if !copy.is_empty() {
        buttons.push(
            button(l, Tone::Ghost, Size::Md, Some(icons::COPY), "Copy details")
                .key("about/copy")
                .on_press(move |s: &mut Studio| s.local.copy = Some(copy.clone())),
        );
    }
    buttons.push(
        button(l, Tone::Primary, Size::Md, None, "Close")
            .key("about/close")
            .on_press(close),
    );
    let panel = dialog_panel(
        l,
        None,
        title,
        col().child(body).child(el().h(4.0)),
        buttons,
        close,
    )
    .w(460.0);
    Some(stillkit::el::dialog(panel).on_dismiss(close))
}
