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
    let mut draw = |r: Box2, color: Color| {
        list.commands.push(Draw::Rect {
            node,
            rect: r,
            color,
        })
    };
    match widget {
        Some(Widget::Toggle(checked)) => {
            draw(
                Box2 {
                    x: rect.x + rect.w - 22.0,
                    y: rect.y + 7.0,
                    w: 14.0,
                    h: 14.0,
                },
                if *checked {
                    accent
                } else {
                    Color(100, 100, 100, 100)
                },
            );
        }
        Some(Widget::Slider {
            value, min, max, ..
        }) => {
            draw(
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
                    draw(
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
                draw(
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
