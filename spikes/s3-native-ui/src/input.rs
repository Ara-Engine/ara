//! Modal focus scopes, directional navigation, IME state and world-surface input.
use crate::{
    runtime::{Ui, is_focusable},
    source::Result,
};
use serde::Serialize;

#[derive(Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}
pub fn next_tab(ui: &Ui, current: usize, reverse: bool) -> Option<usize> {
    let current = ui.paint_order.iter().position(|i| *i == current)?;
    let count = ui.paint_order.len();
    (1..=count)
        .map(|offset| {
            if reverse {
                (current + count - offset) % count
            } else {
                (current + offset) % count
            }
        })
        .map(|position| ui.paint_order[position])
        .find(|i| is_focusable(&ui.nodes[*i]))
}
pub fn directional(ui: &Ui, current: usize, direction: Direction) -> Option<usize> {
    let origin = ui.nodes[current].rect;
    let center = [origin.x + origin.w / 2.0, origin.y + origin.h / 2.0];
    ui.nodes
        .iter()
        .enumerate()
        .filter(|(i, n)| *i != current && is_focusable(n))
        .filter_map(|(i, n)| {
            let dx = n.rect.x + n.rect.w / 2.0 - center[0];
            let dy = n.rect.y + n.rect.h / 2.0 - center[1];
            let (forward, lateral) = match direction {
                Direction::Right => (dx, dy),
                Direction::Left => (-dx, dy),
                Direction::Down => (dy, dx),
                Direction::Up => (-dy, dx),
            };
            (forward > 0.1).then_some((i, forward + lateral.abs() * 3.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|p| p.0)
}
pub fn focus_graph(ui: &Ui) -> serde_json::Value {
    let graph:Vec<_>=ui.nodes.iter().enumerate().filter(|(_,n)|is_focusable(n)).map(|(i,n)| {
        let id=|j:Option<usize>|j.map(|j|ui.nodes[j].element.id.as_str());
        serde_json::json!({"id":n.element.id,"tab":id(next_tab(ui,i,false)),"shift_tab":id(next_tab(ui,i,true)),"left":id(directional(ui,i,Direction::Left)),"right":id(directional(ui,i,Direction::Right)),"up":id(directional(ui,i,Direction::Up)),"down":id(directional(ui,i,Direction::Down))})
    }).collect();
    serde_json::json!(graph)
}
#[derive(Default)]
pub struct FocusStack {
    scopes: Vec<(u64, String)>,
}
impl FocusStack {
    pub fn push(&mut self, screen: u64, initial: &str) {
        self.scopes.push((screen, initial.into()));
    }
    pub fn focus(&mut self, id: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.1.clear();
            scope.1.push_str(id);
        }
    }
    pub fn pop(&mut self) -> Option<&str> {
        self.scopes.pop();
        self.scopes.last().map(|s| s.1.as_str())
    }
}
#[derive(Default, Serialize)]
pub struct Composition {
    pub committed: String,
    pub preedit: String,
    pub selection: std::ops::Range<usize>,
    pub active: bool,
}
pub enum ImeEvent<'a> {
    Start,
    Update {
        preedit: &'a str,
        selection: std::ops::Range<usize>,
    },
    Commit(&'a str),
    Cancel,
}
impl Composition {
    pub fn event(&mut self, event: ImeEvent<'_>) -> Result<()> {
        match event {
            ImeEvent::Start => self.active = true,
            ImeEvent::Update { preedit, selection } => {
                if !self.active
                    || selection.start > selection.end
                    || !preedit.is_char_boundary(selection.start)
                    || !preedit.is_char_boundary(selection.end)
                {
                    return Err("IME001: invalid preedit or selection byte range".into());
                }
                self.preedit.clear();
                self.preedit.push_str(preedit);
                self.selection = selection;
            }
            ImeEvent::Commit(value) => {
                self.committed.push_str(value);
                self.preedit.clear();
                self.selection = 0..0;
                self.active = false;
            }
            ImeEvent::Cancel => {
                self.preedit.clear();
                self.selection = 0..0;
                self.active = false;
            }
        }
        Ok(())
    }
}
/// Intersect a world ray with an orthonormal panel basis, reject back-facing/outside hits, map to logical pixels.
pub fn ray_to_local(
    origin: [f32; 3],
    direction: [f32; 3],
    panel_origin: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    meters: [f32; 2],
    pixels: [f32; 2],
) -> Option<[f32; 2]> {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let normal = [
        right[1] * up[2] - right[2] * up[1],
        right[2] * up[0] - right[0] * up[2],
        right[0] * up[1] - right[1] * up[0],
    ];
    let denom = dot(direction, normal);
    if denom >= -1e-6 || meters[0] <= 0.0 || meters[1] <= 0.0 {
        return None;
    }
    let delta = [
        panel_origin[0] - origin[0],
        panel_origin[1] - origin[1],
        panel_origin[2] - origin[2],
    ];
    let t = dot(delta, normal) / denom;
    if t < 0.0 {
        return None;
    }
    let relative = [
        origin[0] + direction[0] * t - panel_origin[0],
        origin[1] + direction[1] * t - panel_origin[1],
        origin[2] + direction[2] * t - panel_origin[2],
    ];
    let u = dot(relative, right) / meters[0] + 0.5;
    let v = 0.5 - dot(relative, up) / meters[1];
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        None
    } else {
        Some([u * pixels[0], v * pixels[1]])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composition_and_modal_focus_are_restored() -> Result<()> {
        let mut c = Composition::default();
        c.event(ImeEvent::Start)?;
        c.event(ImeEvent::Update {
            preedit: "日本",
            selection: 0..6,
        })?;
        assert_eq!(c.preedit, "日本");
        assert_eq!(c.committed, "");
        c.event(ImeEvent::Commit("日本"))?;
        assert_eq!(c.committed, "日本");
        c.event(ImeEvent::Start)?;
        c.event(ImeEvent::Cancel)?;
        assert!(!c.active);
        let mut stack = FocusStack::default();
        stack.push(1, "resume");
        stack.focus("settings");
        stack.push(2, "volume");
        assert_eq!(stack.pop(), Some("settings"));
        Ok(())
    }
    #[test]
    fn world_ray_rejects_parallel_behind_and_outside() {
        let map = |o, d| {
            ray_to_local(
                o,
                d,
                [0.0; 3],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [2.0, 1.0],
                [640.0, 320.0],
            )
        };
        assert_eq!(map([0.0, 0.0, 2.0], [0.0, 0.0, -1.0]), Some([320.0, 160.0]));
        assert!(map([0.0, 0.0, 2.0], [1.0, 0.0, 0.0]).is_none());
        assert!(map([2.0, 0.0, 2.0], [0.0, 0.0, -1.0]).is_none());
        assert!(map([0.0, 0.0, 2.0], [0.0, 0.0, 1.0]).is_none());
    }
}
