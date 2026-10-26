//! Focus navigation through shown controls and scrollable content in paint order.
use crate::{Ui, input::Direction, runtime::is_focusable};
/// Positive authored tab indices precede document-order controls; negative indices opt out.
pub fn tab_order(ui: &Ui) -> Vec<usize> {
    let mut order: Vec<_> = ui
        .paint_order
        .iter()
        .copied()
        .filter(|i| {
            ui.focus_candidate(*i)
                && ui.nodes[*i]
                    .element
                    .attrs
                    .get("tab-index")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
                    >= 0
        })
        .collect();
    order.sort_by_key(|i| {
        let value = ui.nodes[*i]
            .element
            .attrs
            .get("tab-index")
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
        if value > 0 { value } else { i32::MAX }
    });
    order
}
/// Next shown/enabled control; input reveals clipped content in a scroll view.
pub fn next_tab(ui: &Ui, current: usize, reverse: bool) -> Option<usize> {
    let order = tab_order(ui);
    let pos = order.iter().position(|i| *i == current)?;
    (1..=order.len())
        .map(|n| {
            order[if reverse {
                (pos + order.len() - n) % order.len()
            } else {
                (pos + n) % order.len()
            }]
        })
        .find(|i| ui.focus_candidate(*i))
}
/// Directional navigation with validated explicit authored overrides.
pub fn directional(ui: &Ui, current: usize, direction: Direction) -> Option<usize> {
    directional_where(ui, current, direction, |_| true)
}
pub(crate) fn directional_where(
    ui: &Ui,
    current: usize,
    direction: Direction,
    allowed: impl Fn(usize) -> bool,
) -> Option<usize> {
    let origin = ui.nodes.get(current)?;
    if let Some(target) = origin
        .element
        .attrs
        .get(direction.attribute())
        .and_then(|id| ui.find(id))
        && allowed(target)
        && ui.focus_candidate(target)
    {
        return Some(target);
    }
    let r = origin.rect;
    let center = [r.x + r.w / 2.0, r.y + r.h / 2.0];
    ui.paint_order
        .iter()
        .copied()
        .filter(|i| *i != current && allowed(*i) && ui.focus_candidate(*i))
        .filter_map(|i| {
            let r = ui.nodes[i].rect;
            let dx = r.x + r.w / 2.0 - center[0];
            let dy = r.y + r.h / 2.0 - center[1];
            let (forward, lateral) = match direction {
                Direction::Left => (-dx, dy),
                Direction::Right => (dx, dy),
                Direction::Up => (-dy, dx),
                Direction::Down => (dy, dx),
            };
            (forward > 0.1).then_some((i, forward + lateral.abs() * 3.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}
/// Inspection graph uses actual clipping, enabled states and explicit focus neighbors.
pub fn focus_graph(ui: &Ui) -> serde_json::Value {
    serde_json::json!(ui.paint_order.iter().copied().filter(|i|is_focusable(&ui.nodes[*i])).map(|i|{let id=|j:Option<usize>|j.map(|j|ui.nodes[j].element.id.as_str());serde_json::json!({"id":ui.nodes[i].element.id,"tab":id(next_tab(ui,i,false)),"left":id(directional(ui,i,Direction::Left)),"right":id(directional(ui,i,Direction::Right)),"up":id(directional(ui,i,Direction::Up)),"down":id(directional(ui,i,Direction::Down))})}).collect::<Vec<_>>())
}
