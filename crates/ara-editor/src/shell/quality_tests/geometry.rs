//! Capture-time checks for shaped text and every containing feedback surface.
//! Scroll viewports may intentionally clip scrolled content; ordinary containers may not.
use ara_ui::{Ui, paint::Box2, paint::Draw, source::stable_id};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const EPSILON: f32 = 1.;

/// Read-only evidence shared by mounted-editor tests and native journey captures.
#[derive(Default)]
pub(crate) struct Audit {
    pub(crate) surfaces: Vec<Value>,
    pub(crate) violations: Vec<String>,
}

#[derive(Clone, Copy)]
enum Kind {
    Overlay,
    Feedback,
    Banner,
}

fn kind(ui: &Ui, index: usize) -> Option<Kind> {
    let element = &ui.nodes[index].element;
    if matches!(element.tag.as_str(), "popover" | "tooltip" | "popup") {
        return Some(Kind::Overlay);
    }
    if element.tag == "feedback" || element.id == "drop-label" {
        return Some(Kind::Feedback);
    }
    (element.tag == "banner"
        || element.attrs.get("class").is_some_and(|classes| {
            classes
                .split_whitespace()
                .any(|class| matches!(class, "notice" | "asset-job" | "banner" | "loading-pill"))
        }))
    .then_some(Kind::Banner)
}

fn finite(rect: Box2) -> bool {
    [rect.x, rect.y, rect.w, rect.h]
        .into_iter()
        .all(f32::is_finite)
        && rect.w >= 0.
        && rect.h >= 0.
}

fn contains(outer: Box2, inner: Box2) -> bool {
    finite(outer)
        && finite(inner)
        && inner.x >= outer.x - EPSILON
        && inner.y >= outer.y - EPSILON
        && inner.x + inner.w <= outer.x + outer.w + EPSILON
        && inner.y + inner.h <= outer.y + outer.h + EPSILON
}

fn descendant(ui: &Ui, child: usize, root: usize) -> bool {
    let mut at = Some(child);
    while let Some(index) = at {
        if index == root {
            return true;
        }
        at = ui.nodes[index].element.parent;
    }
    false
}

fn banner_rail_cap(ui: &Ui, root: usize) -> f32 {
    let mut parent = ui.nodes[root].element.parent;
    while let Some(index) = parent {
        if ui.scroll_position(index).is_some() {
            return ui.nodes[index]
                .computed
                .layout
                .max_size
                .height
                .resolve_to_option(ui.size[1], |_, _| 0.)
                .unwrap_or(f32::MAX);
        }
        parent = ui.nodes[index].element.parent;
    }
    f32::MAX
}

/// Use the actual emitted paragraph origin, including the complete paint transform
/// stack. Node rectangles alone cannot detect a subtree translated independently.
fn text_origins(ui: &Ui) -> BTreeMap<u64, [f32; 2]> {
    let mut origins = BTreeMap::new();
    let mut stack = Vec::new();
    let mut translation = [0., 0.];
    for draw in &ui.display.commands {
        match draw {
            Draw::PushTransform { translation: local } => {
                stack.push(translation);
                translation = [translation[0] + local[0], translation[1] + local[1]];
            }
            Draw::PopTransform => translation = stack.pop().unwrap_or([0., 0.]),
            Draw::GlyphRun { node, origin, .. } | Draw::ScalableGlyphRun { node, origin, .. } => {
                origins
                    .entry(*node)
                    .or_insert([origin[0] + translation[0], origin[1] + translation[1]]);
            }
            _ => {}
        }
    }
    origins
}

fn ancestors(audit: &mut Audit, ui: &Ui, root: usize, child: usize, mut bounds: Box2, what: &str) {
    let mut parent = ui.nodes[child].element.parent;
    while let Some(index) = parent {
        let ancestor = &ui.nodes[index];
        if ui.scroll_position(index).is_some() {
            // Only explicit scroll widgets authorize clipping. Continue checking
            // the visible portion against every ordinary ancestor above them.
            bounds = bounds.intersection(ancestor.rect);
            if bounds.w <= 0. || bounds.h <= 0. {
                break;
            }
        } else if !contains(ancestor.rect, bounds) {
            audit.violations.push(format!(
                "{}: {} {what} escapes ancestor {}: {bounds:?} outside {:?}",
                ui.nodes[root].element.id,
                ui.nodes[child].element.id,
                ancestor.element.id,
                ancestor.rect
            ));
        }
        if index == root {
            break;
        }
        parent = ancestor.element.parent;
    }
}

/// Measure the last rendered logical geometry, without changing UI state or files.
/// Width limits apply to transient overlays and drag feedback, never wide banners.
pub(crate) fn audit(ui: &Ui) -> Audit {
    let mut audit = Audit::default();
    let origins = text_origins(ui);
    let viewport = Box2 {
        x: 0.,
        y: 0.,
        w: ui.size[0],
        h: ui.size[1],
    };
    for (root, node) in ui.nodes.iter().enumerate() {
        let Some(kind) = kind(ui, root) else { continue };
        if !ui.shown(root) {
            continue;
        }
        let maximum = match kind {
            Kind::Overlay if node.element.id == "palette" => Some(640.),
            Kind::Overlay | Kind::Feedback => Some(ara_ui::popover::MAX_WIDTH),
            Kind::Banner => None,
        };
        if !finite(node.rect) || node.rect.w <= 0. || node.rect.h <= 0. {
            audit.violations.push(format!(
                "{} has invalid feedback bounds: {:?}",
                node.element.id, node.rect
            ));
        }
        if maximum.is_some_and(|maximum| node.rect.w > maximum + 0.75) {
            audit.violations.push(format!(
                "{} too wide: {} > {}",
                node.element.id,
                node.rect.w,
                maximum.unwrap_or_default()
            ));
        }
        if !matches!(kind, Kind::Banner) && !contains(viewport, node.rect) {
            audit.violations.push(format!(
                "{} escapes viewport: {:?}",
                node.element.id, node.rect
            ));
        }
        // The first card must fit as a complete feedback unit, not expose a title
        // while clipping its result and action. Later scrolled cards may be partial.
        if matches!(kind, Kind::Banner)
            && (node.rect.y - node.clip.y).abs() <= EPSILON
            && node.rect.h > node.clip.h + EPSILON
            && node.clip.h + EPSILON < banner_rail_cap(ui, root)
        {
            audit.violations.push(format!(
                "{} first visible banner clips vertically: {:?} in {:?}",
                node.element.id, node.rect, node.clip
            ));
        }
        let mut texts = Vec::new();
        for (index, child) in ui.nodes.iter().enumerate() {
            if !ui.shown(index) || !descendant(ui, index, root) {
                continue;
            }
            if index != root {
                ancestors(&mut audit, ui, root, index, child.rect, "layout");
            }
            let Some(text) = &child.text else { continue };
            if text.value.is_empty() {
                continue;
            }
            let Ok(layout) = ui.tree.layout(child.layout_id) else {
                audit.violations.push(format!(
                    "{}: {} has no rendered layout",
                    node.element.id, child.element.id
                ));
                continue;
            };
            let content = Box2 {
                x: child.rect.x + layout.padding.left + layout.border.left,
                y: child.rect.y + layout.padding.top + layout.border.top,
                w: (layout.size.width
                    - layout.padding.left
                    - layout.padding.right
                    - layout.border.left
                    - layout.border.right)
                    .max(0.),
                h: (layout.size.height
                    - layout.padding.top
                    - layout.padding.bottom
                    - layout.border.top
                    - layout.border.bottom)
                    .max(0.),
            };
            let emitted = origins.get(&stable_id(&child.element.id));
            let origin = emitted.copied().unwrap_or([content.x, content.y]);
            let paragraph = Box2 {
                x: origin[0],
                y: origin[1],
                w: text.layout.width(),
                h: text.layout.height(),
            };
            if !contains(content, paragraph) {
                audit.violations.push(format!("{}: {} shaped text clips within its own content box: {paragraph:?} outside {content:?}", node.element.id, child.element.id));
            }
            if index != root {
                ancestors(&mut audit, ui, root, index, paragraph, "shaped text");
            }
            texts.push(json!({"id":child.element.id,"paragraph":paragraph,"content":content,"emitted":emitted.is_some()}));
        }
        audit.surfaces.push(json!({"surface":node.element.id,"kind":match kind {Kind::Overlay=>"overlay",Kind::Feedback=>"feedback",Kind::Banner=>"banner"},"box":node.rect,"clip":node.clip,"viewport":ui.size,"maxWidth":maximum,"text":texts}));
    }
    audit
}
