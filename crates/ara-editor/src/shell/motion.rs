//! Paint-only 140 ms colour transitions; settled frames do no animation work.
use ara_ui::{
    paint::{DisplayList, Draw},
    style::Color,
};
use std::{collections::BTreeMap, time::Instant};
#[derive(Default)]
pub(super) struct Motion {
    colors: BTreeMap<(u64, u8), Color>,
    tweens: BTreeMap<(u64, u8), Tween>,
}
struct Tween {
    from: Color,
    to: Color,
    start: Instant,
}
impl Motion {
    pub fn apply(&mut self, list: &mut DisplayList, fresh: bool, reduce: bool) -> bool {
        if reduce {
            self.tweens.clear();
            if !fresh {
                return false;
            }
        }
        if !fresh && self.tweens.is_empty() {
            return false;
        }
        let was_active = !self.tweens.is_empty();
        if self.colors.len() > list.commands.len() * 2 {
            self.colors.clear();
        }
        let now = Instant::now();
        let mut active = false;
        for draw in &mut list.commands {
            let (key, color) = match draw {
                Draw::RoundedRect { node, color, .. } | Draw::Rect { node, color, .. } => {
                    ((*node, 0), color)
                }
                Draw::Border { node, color, .. } => ((*node, 1), color),
                _ => continue,
            };
            if fresh {
                if let Some(old) = self.colors.insert(key, *color)
                    && old != *color
                    && !reduce
                {
                    let from = self.tweens.get(&key).map_or(old, |t| sample(t, now));
                    self.tweens.insert(
                        key,
                        Tween {
                            from,
                            to: *color,
                            start: now,
                        },
                    );
                }
            }
            if let Some(t) = self.tweens.get(&key) {
                *color = sample(t, now);
                active |= now.duration_since(t.start).as_millis() < 140;
            }
        }
        self.tweens
            .retain(|_, t| now.duration_since(t.start).as_millis() < 140);
        active || was_active
    }
}
fn sample(t: &Tween, now: Instant) -> Color {
    let p = (now.duration_since(t.start).as_secs_f32() / 0.14).clamp(0., 1.);
    let p = 1. - (1. - p).powi(3);
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * p).round() as u8;
    Color(
        mix(t.from.0, t.to.0),
        mix(t.from.1, t.to.1),
        mix(t.from.2, t.to.2),
        mix(t.from.3, t.to.3),
    )
}
