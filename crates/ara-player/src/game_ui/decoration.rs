//! Presentation-only scrims use native paint commands over the live scene image.
use super::*;

pub(super) fn apply(ui: &mut Ui, screen: Screen) {
    if screen != Screen::Main {
        return;
    }
    let [width, height] = ui.size;
    let mut commands = Vec::with_capacity(128);
    for slice in 0..128 {
        let t = slice as f32 / 128.;
        let start = (width * t).floor();
        let end = (width * (slice + 1) as f32 / 128.).floor();
        let alpha = (238. * (1. - t).powf(1.6) + 12.) as u8;
        commands.push(Draw::Rect {
            node: u64::MAX,
            rect: Box2 {
                x: start,
                y: 0.,
                w: end - start,
                h: height,
            },
            color: ara_ui::style::Color(7, 15, 19, alpha),
        });
    }
    ui.display
        .commands
        .retain(|command| !matches!(command, Draw::Rect { node: u64::MAX, .. }));
    ui.display.commands.splice(0..0, commands);
}
