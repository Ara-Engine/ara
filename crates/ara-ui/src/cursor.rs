//! Platform-neutral cursor hints resolved from visible hover and pointer capture.
use crate::{Ui, input::InputState, source::stable_id};

/// Pointer shape; native hosts translate this into their platform cursor API.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    /// Standard selection arrow.
    #[default]
    Arrow,
    /// Editable text.
    Text,
    /// Clickable control.
    Pointer,
    /// Horizontal resize.
    ResizeEw,
    /// Vertical resize.
    ResizeNs,
    /// Northwest/southeast resize.
    ResizeNwse,
    /// Northeast/southwest resize.
    ResizeNesw,
    /// Move in any direction.
    Move,
    /// Disabled operation.
    NotAllowed,
    /// Draggable content.
    Grab,
    /// Captured drag.
    Grabbing,
}
impl Cursor {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "default" | "arrow" => Self::Arrow,
            "text" => Self::Text,
            "pointer" | "hand" => Self::Pointer,
            "ew-resize" => Self::ResizeEw,
            "ns-resize" => Self::ResizeNs,
            "nwse-resize" => Self::ResizeNwse,
            "nesw-resize" => Self::ResizeNesw,
            "move" => Self::Move,
            "not-allowed" => Self::NotAllowed,
            "grab" => Self::Grab,
            "grabbing" => Self::Grabbing,
            _ => return None,
        })
    }
}
impl Ui {
    /// Resolve `cursor`/`active-cursor` attributes, widget defaults, and captured drags.
    /// Hover follows paint order and clipping; a captured cursor survives leaving its widget.
    pub fn cursor(&self, input: &InputState, position: [f32; 2]) -> Cursor {
        if input.scrolling() {
            return Cursor::Arrow;
        }
        let captured = input
            .captured()
            .and_then(|id| self.by_id(id))
            .filter(|i| self.shown(*i));
        let mut index = captured.or_else(|| {
            self.paint_order.iter().copied().rev().find(|i| {
                let n = &self.nodes[*i];
                n.rect.contains(position[0], position[1])
                    && n.clip.contains(position[0], position[1])
                    && self.rounded_hit(*i, position[0], position[1])
                    && input.in_scope(self, *i)
            })
        });
        if captured.is_none() && self.scrollbar_at(position).is_some() {
            return Cursor::Arrow;
        }
        while let Some(i) = index {
            let n = &self.nodes[i];
            if n.state.disabled {
                return Cursor::NotAllowed;
            }
            if captured == Some(i) {
                if let Some(cursor) = n
                    .element
                    .attrs
                    .get("active-cursor")
                    .and_then(|s| Cursor::parse(s))
                {
                    return cursor;
                }
                if n.element.attrs.get("cursor").is_some_and(|s| s == "grab") {
                    return Cursor::Grabbing;
                }
            }
            if let Some(cursor) = n.element.attrs.get("cursor").and_then(|s| Cursor::parse(s)) {
                return cursor;
            }
            match n.element.tag.as_str() {
                "input" | "field" => return Cursor::Text,
                "button" | "toggle" | "tab" | "dropdown" | "menu" | "link" => {
                    return Cursor::Pointer;
                }
                "slider" => return Cursor::ResizeEw,
                "splitter" => {
                    let horizontal = n
                        .element
                        .parent
                        .and_then(|p| self.tree.style(self.nodes[p].layout_id).ok())
                        .is_some_and(|s| {
                            matches!(
                                s.flex_direction,
                                taffy::FlexDirection::Row | taffy::FlexDirection::RowReverse
                            )
                        });
                    return if horizontal {
                        Cursor::ResizeEw
                    } else {
                        Cursor::ResizeNs
                    };
                }
                _ => {}
            }
            index = n.element.parent;
        }
        Cursor::Arrow
    }
    /// Current retained scroll position, including virtual view offsets.
    pub fn scroll_position(&self, index: usize) -> Option<[f32; 2]> {
        match self
            .widgets
            .get(&stable_id(&self.nodes.get(index)?.element.id))
        {
            Some(crate::widgets::Widget::Scroll(p)) => Some(*p),
            _ => None,
        }
    }
}
