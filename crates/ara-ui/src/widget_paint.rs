//! Selection, caret, preedit and native control paint, using Parley bidi geometry.
use crate::{
    paint::{Box2, DisplayList, Draw},
    style::Color,
    widgets::Widget,
};
#[allow(
    clippy::too_many_arguments,
    reason = "Paint emission passes borrowed widget/text state without cloning retained nodes"
)]
pub(crate) fn decorations(
    widget: Option<&Widget>,
    text: Option<&ara_ui_text::Text>,
    focused: bool,
    rect: Box2,
    origin: [f32; 2],
    node: u64,
    accent: Color,
    list: &mut DisplayList,
) {
    match widget {
        Some(Widget::Toggle(checked)) => {
            // A 14 px rounded check box, centred in toggles up to 28 px wide and
            // right-aligned (7 px inset) in wider ones. Neutral translucent colours read on both
            // themes; checked boxes fill with the accent and carry an antialiased tick.
            let r = Box2 {
                x: if rect.w <= 28.0 {
                    rect.x + (rect.w - 14.0) * 0.5
                } else {
                    rect.x + rect.w - 21.0
                },
                y: rect.y + ((rect.h - 14.0) * 0.5).max(0.0),
                w: 14.0,
                h: 14.0,
            };
            let (fill, ring) = if *checked {
                (accent, Color(255, 255, 255, 46))
            } else {
                (Color(128, 128, 128, 34), Color(128, 128, 128, 120))
            };
            list.commands.push(Draw::RoundedRect {
                node,
                rect: r,
                radius: 4.0,
                color: fill,
            });
            list.commands.push(Draw::Border {
                node,
                rect: r,
                radius: 4.0,
                width: 1.0,
                color: ring,
            });
            if *checked {
                for (a, b) in [([3.6, 7.2], [6.0, 9.6]), ([6.0, 9.6], [10.6, 4.6])] {
                    list.commands.push(Draw::Line {
                        node,
                        from: [r.x + a[0], r.y + a[1]],
                        to: [r.x + b[0], r.y + b[1]],
                        width: 1.8,
                        color: Color(255, 255, 255, 255),
                    });
                }
            }
        }
        Some(Widget::Slider {
            value, min, max, ..
        }) => {
            fill(
                list,
                node,
                Box2 {
                    x: rect.x + 6.0,
                    y: rect.y + rect.h / 2.0 - 2.0,
                    w: (rect.w - 12.0) * ((value - min) / (max - min)) as f32,
                    h: 4.0,
                },
                accent,
            );
        }
        Some(Widget::Input(input)) if focused => {
            if let Some(text) = text {
                use parley::{Affinity, Cursor, Selection};
                let r = input.preedit_range().unwrap_or_else(|| input.selection());
                let selection = Selection::new(
                    Cursor::from_byte_index(&text.layout, r.start, Affinity::Downstream),
                    Cursor::from_byte_index(&text.layout, r.end, Affinity::Upstream),
                );
                selection.geometry_with(&text.layout, |b, _| {
                    fill(
                        list,
                        node,
                        Box2 {
                            x: origin[0] + b.x0 as f32,
                            y: origin[1]
                                + if input.preedit_range().is_some() {
                                    b.y1 as f32 - 1.0
                                } else {
                                    b.y0 as f32
                                },
                            w: (b.x1 - b.x0) as f32,
                            h: if input.preedit_range().is_some() {
                                1.0
                            } else {
                                (b.y1 - b.y0) as f32
                            },
                        },
                        Color(accent.0, accent.1, accent.2, 100),
                    );
                });
                let c = Cursor::from_byte_index(
                    &text.layout,
                    input.display_caret(),
                    Affinity::Downstream,
                )
                .geometry(&text.layout, 1.0);
                fill(
                    list,
                    node,
                    Box2 {
                        x: origin[0] + c.x0 as f32,
                        y: origin[1] + c.y0 as f32,
                        w: 1.0,
                        h: (c.y1 - c.y0) as f32,
                    },
                    accent,
                );
            }
        }
        _ => {}
    }
}

/// Push one solid rectangle for `node`.
fn fill(list: &mut DisplayList, node: u64, rect: Box2, color: Color) {
    list.commands.push(Draw::Rect { node, rect, color });
}
