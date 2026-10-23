use crate::{Result, query, report::*};
use ara_ui::{
    Ui,
    paint::{Box2, Draw},
    style::Color,
};
use ara_ui_render::gpu::Renderer;
use ara_ui_stories::StoryView;

pub(crate) fn inspect(ui: &Ui, view: &StoryView, renderer: &Renderer) -> Result<CaptureReport> {
    let mut out = CaptureReport {
        version: 1,
        view: view.clone(),
        pixels: view.physical_size()?.to_vec(),
        commands: ui.display.commands.len() as u32,
        glyphs: ui.display.glyphs.len() as u32,
        focus_graph: serde_json::from_value(ara_ui::input::focus_graph(ui))?,
        ..CaptureReport::default()
    };
    for (i, n) in ui.nodes.iter().enumerate() {
        let visible = query::visible(ui, i);
        let l = ui.tree.layout(n.layout_id)?;
        let text_bounds = n.text.as_ref().map(|text| Box2 {
            x: n.rect.x + l.padding.left + l.border.left,
            y: n.rect.y + l.padding.top + l.border.top,
            w: text.layout.width(),
            h: text.layout.height(),
        });
        out.nodes.push(NodeReport {
            id: n.element.id.clone(),
            tag: n.element.tag.clone(),
            parent: n.element.parent.map(|p| ui.nodes[p].element.id.clone()),
            role: query::role(ui, i).into(),
            label: query::label(ui, i).into(),
            visible,
            text: n.text.as_ref().map(|t| t.value.clone()),
            value: query::value(ui, i),
            bounds: n.rect.into(),
            clip: n.clip.into(),
            text_bounds: text_bounds.map(Into::into),
            provenance: ui.sheet.provenance(&n.element, n.state),
            shaping: n
                .text
                .as_ref()
                .map(|t| ara_json::to_value(&ara_ui_text::inspect(t)))
                .transpose()?,
            style: ComputedStyle {
                background: rgba(n.computed.background),
                foreground: rgba(n.computed.color),
                border_color: rgba(n.computed.border_color),
                font_size: n.computed.font_size,
                radius: n.computed.radius,
                border_width: n.computed.border,
                display: format!("{:?}", ui.tree.style(n.layout_id)?.display),
                flex_direction: format!("{:?}", n.computed.layout.flex_direction),
                clips: n.computed.clip,
                padding: vec![
                    l.padding.top,
                    l.padding.right,
                    l.padding.bottom,
                    l.padding.left,
                ],
            },
        });
        if !visible {
            continue;
        }
        let boundary = n
            .element
            .parent
            .map_or(n.clip, |parent| ui.nodes[parent].rect);
        if outside(n.rect, boundary) {
            warn(
                &mut out,
                "ARA7100",
                &n.element.id,
                format!(
                    "{}: box overflows {} in {}",
                    n.element.id,
                    if n.element.parent.is_some() {
                        "parent"
                    } else {
                        "viewport"
                    },
                    view.locale
                ),
                "Allow wrapping, increase the container, use flexible tracks or add intentional scrolling",
            );
        }
        if let Some(text) = &n.text {
            let content = Box2 {
                x: n.rect.x + l.padding.left + l.border.left,
                y: n.rect.y + l.padding.top + l.border.top,
                w: (n.rect.w - l.padding.left - l.padding.right - l.border.left - l.border.right)
                    .max(0.0),
                h: (n.rect.h - l.padding.top - l.padding.bottom - l.border.top - l.border.bottom)
                    .max(0.0),
            };
            let bounds = text_bounds.expect("text bounds exist for a text node");
            if outside(bounds, content) || outside(bounds, n.clip) {
                warn(
                    &mut out,
                    "ARA7101",
                    &n.element.id,
                    format!(
                        "Text exceeds content or clip bounds in {} ({} x {})",
                        view.locale, bounds.w, bounds.h
                    ),
                    "Allow multiline text, expand the content box or provide scrolling; keep accessibility font size",
                );
            }
            let evidence = ara_ui_text::inspect(text);
            if evidence.missing_glyphs > 0 {
                warn(
                    &mut out,
                    "ARA7102",
                    &n.element.id,
                    format!(
                        "{} missing glyphs in {}",
                        evidence.missing_glyphs, view.locale
                    ),
                    "Supply a licensed font pack covering these Unicode clusters",
                );
            }
            let id = ara_ui::source::stable_id(&n.element.id);
            if ui.display.commands.iter().any(|draw| matches!(draw, Draw::GlyphRun { node, font, .. } if *node == id && *font != 0)) {
                warn(&mut out, "ARA7103", &n.element.id, format!("Text uses an explicit fallback font in {}", view.locale), "Inspect shaping clusters; retain the required fallback pack or select a covering primary font");
            }
            if !text.value.is_empty() {
                contrast(ui, i, &mut out);
            }
        }
        if ara_ui::runtime::is_focusable(n) {
            let target = n.rect.intersection(n.clip);
            let minimum = target.w.min(target.h);
            if minimum < 44.0 {
                warn(
                    &mut out,
                    "ARA7106",
                    &n.element.id,
                    format!(
                        "Effective touch target is {:.1} x {:.1} logical pixels",
                        target.w, target.h
                    ),
                    "Increase interactive padding/minimum dimensions to at least 44 x 44 logical pixels",
                );
                let d = out
                    .diagnostics
                    .last_mut()
                    .expect("warn appended a diagnostic");
                d.measured = Some(f64::from(minimum));
                d.threshold = Some(44.0);
            }
        }
    }
    out.batches = renderer
        .batches
        .iter()
        .map(|b| BatchReport {
            pipeline: format!("{:?}", b.pipeline),
            texture: b.texture,
            clip: b.clip as u32,
            first: b.instances.start as u32,
            count: b.instances.len() as u32,
            cause: b.break_cause.into(),
        })
        .collect();
    Ok(out)
}
fn rgba(c: Color) -> Vec<u8> {
    vec![c.0, c.1, c.2, c.3]
}
fn outside(r: Box2, p: Box2) -> bool {
    r.x < p.x - 0.5 || r.y < p.y - 0.5 || r.x + r.w > p.x + p.w + 0.5 || r.y + r.h > p.y + p.h + 0.5
}
fn warn(out: &mut CaptureReport, code: &str, node: &str, message: impl Into<String>, fix: &str) {
    out.diagnostics.push(UiDiagnostic {
        code: code.into(),
        node: node.into(),
        message: message.into(),
        suggested_fix: fix.into(),
        ..UiDiagnostic::default()
    });
}
fn over(fg: [f64; 4], bg: [f64; 4]) -> [f64; 4] {
    let a = fg[3] + bg[3] * (1.0 - fg[3]);
    if a == 0.0 {
        return [0.0; 4];
    }
    [
        (fg[0] * fg[3] + bg[0] * bg[3] * (1.0 - fg[3])) / a,
        (fg[1] * fg[3] + bg[1] * bg[3] * (1.0 - fg[3])) / a,
        (fg[2] * fg[3] + bg[2] * bg[3] * (1.0 - fg[3])) / a,
        a,
    ]
}
fn luminance(c: [f64; 4]) -> f64 {
    let linear = |n: f64| {
        if n <= 0.04045 {
            n / 12.92
        } else {
            ((n + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
}
fn contrast(ui: &Ui, i: usize, out: &mut CaptureReport) {
    let n = &ui.nodes[i];
    let mut chain = Vec::new();
    let mut node = Some(i);
    while let Some(j) = node {
        chain.push(j);
        node = ui.nodes[j].element.parent;
    }
    let mut background = [0.0; 4];
    let channels = |c: Color| [c.0, c.1, c.2, c.3].map(|v| f64::from(v) / 255.0);
    for j in chain.into_iter().rev() {
        background = over(channels(ui.nodes[j].computed.background), background);
    }
    // Use the effective mirrored node boxes, not untransformed display-list image rectangles.
    let image = ui
        .paint_order
        .iter()
        .any(|j| ui.nodes[*j].element.tag == "image" && ui.nodes[*j].rect.intersects(n.rect));
    if image || background[3] < 0.999 {
        warn(
            out,
            "ARA7105",
            &n.element.id,
            "Contrast is uncertain over imagery or an unbacked transparent surface",
            "Provide a solid scrim/background or inspect representative game imagery",
        );
        out.diagnostics
            .last_mut()
            .expect("warn appended a diagnostic")
            .uncertain = true;
        return;
    }
    let a = luminance(over(channels(n.computed.color), background));
    let b = luminance(background);
    let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
    let threshold = if n.computed.font_size >= 24.0 {
        3.0
    } else {
        4.5
    };
    // Report passes as well: agents need the actual ratio, not only failing checks.
    out.diagnostics.push(UiDiagnostic {
        code: "ARA7104".into(),
        node: n.element.id.clone(),
        message: format!(
            "Solid-background contrast {:.2}:1 ({})",
            ratio,
            if ratio >= threshold { "pass" } else { "fail" }
        ),
        suggested_fix: if ratio < threshold {
            "Select foreground/background semantic tokens meeting the WCAG threshold".into()
        } else {
            String::new()
        },
        measured: Some(ratio),
        threshold: Some(threshold),
        uncertain: false,
    });
}
