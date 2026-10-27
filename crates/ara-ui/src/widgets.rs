//! Control state and behavior; all values stay in the native runtime until emitted as actions.
use crate::{
    Result, Ui,
    binding::Action,
    source::{Element, stable_id},
};
use ara_ui_text::{Clipboard, EditEvent, TextInput};
use std::collections::BTreeMap;
/// Widget-specific state. Containers use ordinary retained children.
pub enum Widget {
    /// Single-line editable text, selection, undo and IME state.
    Input(TextInput),
    /// Boolean control value.
    Toggle(bool),
    /// Finite numeric value with validated bounds and quantization.
    Slider {
        /// Current value.
        value: f64,
        /// Lower limit.
        min: f64,
        /// Upper limit.
        max: f64,
        /// Positive increment.
        step: f64,
    },
    /// Active tab, dropdown or menu item.
    Choice {
        /// Selected child index.
        selected: usize,
        /// Whether its popup is open.
        open: bool,
    },
    /// Scroll position applied to the content, never the viewport.
    Scroll([f32; 2]),
    /// User-controlled splitter ratio.
    Splitter(f64),
}
impl Widget {
    pub(crate) fn parse(e: &Element) -> Result<Option<Self>> {
        let number = |key: &str, default: f64| -> Result<f64> {
            let v = e.attrs.get(key).map_or(Ok(default), |v| v.parse())?;
            if !v.is_finite() {
                return Err("ARA4044: nonfinite widget number".into());
            }
            Ok(v)
        };
        Ok(match e.tag.as_str() {
            "input" | "field" => Some(Self::Input(TextInput::new(
                e.attrs.get("text").map(String::as_str).unwrap_or_default(),
            ))),
            "toggle" => Some(Self::Toggle(
                e.attrs.get("checked").is_some_and(|v| v == "true"),
            )),
            "slider" | "progress" => {
                let min = number("min", 0.0)?;
                let max = number("max", 1.0)?;
                let step = number("step", 0.01)?;
                let value = number("value", min)?;
                if min >= max || step <= 0.0 || value < min || value > max {
                    return Err("ARA4044: invalid slider range, step or value".into());
                }
                Some(Self::Slider {
                    value,
                    min,
                    max,
                    step,
                })
            }
            "tabs" | "dropdown" | "menu" => Some(Self::Choice {
                selected: e
                    .attrs
                    .get("selected")
                    .map_or(Ok(0), |v| v.parse::<usize>())?,
                open: false,
            }),
            // A popover clamped to the window scrolls its content.
            "scroll" | "popover" => Some(Self::Scroll([0.0; 2])),
            "splitter" => Some(Self::Splitter(0.5)),
            _ => None,
        })
    }
}
impl Ui {
    /// Hit-test a shaped bidi paragraph to place or extend a grapheme-safe selection.
    pub fn select_text_at(
        &mut self,
        index: usize,
        position: [f32; 2],
        extend: bool,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        let id = self.node_id(index)?;
        let Some(Widget::Input(input)) = self.widgets.get(&id) else {
            return Ok(());
        };
        let n = &self.nodes[index];
        let layout = self.tree.layout(n.layout_id)?;
        let text = n.text.as_ref().ok_or("ARA4044: field text is absent")?;
        let scroll = input_scroll(
            input,
            text,
            (layout.size.width
                - layout.padding.left
                - layout.padding.right
                - layout.border.left
                - layout.border.right
                - if crate::input::numeric::declared(&n.element) {
                    crate::input::numeric::GRIP
                } else {
                    0.
                })
            .max(1.0),
        );
        let cursor = parley::Cursor::from_point(
            &text.layout,
            position[0] - n.rect.x - layout.padding.left - layout.border.left + scroll,
            position[1] - n.rect.y - layout.padding.top - layout.border.top,
        );
        let caret = cursor.index();
        let anchor = if extend { input.anchor() } else { caret };
        self.edit(index, EditEvent::Select { anchor, caret }, clipboard)
    }
    pub(crate) fn sync_choices(&mut self) -> Result<()> {
        for index in 0..self.nodes.len() {
            let id = stable_id(&self.nodes[index].element.id);
            let Some(Widget::Choice { selected, open }) = self.widgets.get(&id) else {
                continue;
            };
            let tabs = self.nodes[index].element.tag == "tabs";
            for position in 0..self.nodes[index].element.children.len() {
                let child = self.nodes[index].element.children[position];
                let node = &mut self.nodes[child];
                let visible = if tabs { position == *selected } else { *open };
                node.visible = visible;
                let display = if visible {
                    node.computed.layout.display
                } else {
                    taffy::Display::None
                };
                let current = self.tree.style(node.layout_id)?;
                if current.display != display {
                    let mut style = current.clone();
                    style.display = display;
                    self.tree.set_style(node.layout_id, style)?;
                    node.dirty.layout = true;
                    node.dirty.paint = true;
                }
            }
        }
        Ok(())
    }
    /// Look up a stable authored/model-key identity; recycled pool slots are never identities.
    pub fn by_id(&self, id: u64) -> Option<usize> {
        self.lookup(id, |n| stable_id(&n.element.id) == id)
    }
    /// Push a model value into a toggle without emitting a user action.
    pub fn set_checked(&mut self, index: usize, value: bool) -> Result<()> {
        let id = self.node_id(index)?;
        let Some(Widget::Toggle(v)) = self.widgets.get_mut(&id) else {
            return Err("ARA4044: bool binding requires a toggle".into());
        };
        if *v != value {
            *v = value;
            self.nodes[index].dirty.paint = true;
        }
        Ok(())
    }
    /// Push a model value into a slider/splitter, clamping and snapping to its declared range.
    pub fn set_value(&mut self, index: usize, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err("ARA4044: nonfinite widget value".into());
        }
        let id = self.node_id(index)?;
        match self.widgets.get_mut(&id) {
            Some(Widget::Slider {
                value: v,
                min,
                max,
                step,
            }) => {
                *v = (*min + ((value - *min) / *step).round() * *step).clamp(*min, *max);
            }
            Some(Widget::Splitter(v)) => *v = value.clamp(0.1, 0.9),
            _ => return Err("ARA4044: numeric binding requires a slider or splitter".into()),
        }
        if let Some(Widget::Splitter(ratio)) = self.widgets.get(&id) {
            let ratio = *ratio as f32;
            if let Some(parent) = self.nodes[index].element.parent
                && let Some(position) = self.nodes[parent]
                    .element
                    .children
                    .iter()
                    .position(|i| *i == index)
                && position > 0
                && position + 1 < self.nodes[parent].element.children.len()
            {
                let left = self.nodes[parent].element.children[position - 1];
                let right = self.nodes[parent].element.children[position + 1];
                for (child, grow) in [(left, ratio), (right, 1.0 - ratio)] {
                    let node = &mut self.nodes[child];
                    let mut style = self.tree.style(node.layout_id)?.clone();
                    style.flex_grow = grow;
                    self.tree.set_style(node.layout_id, style)?;
                    node.computed.layout.flex_grow = grow;
                    node.dirty.layout = true;
                }
            }
        }
        self.nodes[index].dirty.paint = true;
        Ok(())
    }
    /// Change a tab/dropdown selection on input and emit its named action.
    pub fn select(&mut self, index: usize, selected: usize) -> Result<()> {
        let id = self.node_id(index)?;
        let Some(Widget::Choice {
            selected: value,
            open,
        }) = self.widgets.get_mut(&id)
        else {
            return Err("ARA4044: control is not a choice".into());
        };
        if selected >= self.nodes[index].element.children.len() {
            return Err("ARA4044: choice index out of range".into());
        }
        *value = selected;
        *open = false;
        self.nodes[index].dirty.layout = true;
        self.nodes[index].dirty.paint = true;
        self.emit(index, serde_json::json!(selected));
        Ok(())
    }
    /// Apply a pointer/gamepad activation and return control values through named actions.
    pub fn activate(&mut self, index: usize) -> Result<()> {
        if self.nodes.get(index).is_none_or(|n| n.state.disabled) {
            return Ok(());
        }
        let id = self.node_id(index)?;
        let value = match self.widgets.get_mut(&id) {
            Some(Widget::Toggle(v)) => {
                *v = !*v;
                serde_json::json!(*v)
            }
            Some(Widget::Choice { open, .. }) => {
                *open = !*open;
                serde_json::json!(*open)
            }
            _ => serde_json::Value::Null,
        };
        self.nodes[index].dirty.paint = true;
        self.emit(index, value);
        Ok(())
    }
    /// OS/keyboard editing seam. Preedit is visual-only; committed edits emit named actions.
    pub fn edit(
        &mut self,
        index: usize,
        event: EditEvent,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        let id = self.node_id(index)?;
        let Some(Widget::Input(input)) = self.widgets.get_mut(&id) else {
            return Err("ARA4044: control is not a text input".into());
        };
        let before = input.value().to_owned();
        let changed = input.event(event, clipboard)?;
        let display = input.display();
        let committed = input.value().to_owned();
        if changed {
            self.set_display_text(index, &display)?;
            self.nodes[index].dirty.paint = true;
        }
        if before != committed {
            self.emit(index, serde_json::json!(committed));
        }
        Ok(())
    }
    /// Move a text caret visually through bidi runs, preserving logical UTF-8 selection.
    pub fn move_caret(
        &mut self,
        index: usize,
        right: bool,
        select: bool,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        use parley::{Affinity, Cursor};
        let id = self.node_id(index)?;
        let Some(Widget::Input(input)) = self.widgets.get(&id) else {
            return Err("ARA4044: not a field".into());
        };
        let text = self.nodes[index]
            .text
            .as_ref()
            .ok_or("ARA4044: missing field text")?;
        let cursor = Cursor::from_byte_index(&text.layout, input.caret(), Affinity::Downstream);
        let next = if right {
            cursor.next_visual(&text.layout)
        } else {
            cursor.previous_visual(&text.layout)
        };
        let selection = input.selection();
        let anchor = if select {
            if input.caret() == selection.start {
                selection.end
            } else {
                selection.start
            }
        } else {
            next.index()
        };
        self.edit(
            index,
            EditEvent::Select {
                anchor,
                caret: next.index(),
            },
            clipboard,
        )
    }
    /// Drain queued host actions once per UI/simulation boundary.
    pub fn drain_actions(&mut self) -> impl Iterator<Item = Action> + '_ {
        self.actions.drain(..)
    }
    pub(crate) fn node_id(&self, index: usize) -> Result<u64> {
        self.nodes
            .get(index)
            .map(|n| stable_id(&n.element.id))
            .ok_or_else(|| "ARA4044: invalid node handle".into())
    }
    pub(crate) fn emit(&mut self, index: usize, value: serde_json::Value) {
        if let Some(name) = self.nodes[index].element.attrs.get("action") {
            self.actions.push(Action {
                name: name.clone(),
                node: stable_id(&self.nodes[index].element.id),
                value,
            });
        }
    }
    /// Scroll retained content without changing its geometry or the viewport clip.
    pub fn scroll_to(&mut self, index: usize, position: [f32; 2]) -> Result<()> {
        if position.iter().any(|v| !v.is_finite()) {
            return Err("ARA4044: invalid scroll position".into());
        }
        let id = self.node_id(index)?;
        if !matches!(self.widgets.get(&id), Some(Widget::Scroll(_))) {
            return Err("ARA4044: not a scroll view".into());
        }
        let node = &self.nodes[index];
        let viewport = self.tree.layout(node.layout_id)?;
        let extent = self.scroll_extent(index)?;
        let target = [
            position[0].clamp(0.0, (extent[0] - viewport.size.width).max(0.0)),
            position[1].clamp(0.0, (extent[1] - viewport.size.height).max(0.0)),
        ];
        if let Some(Widget::Scroll(p)) = self.widgets.get_mut(&id)
            && *p != target
        {
            *p = target;
            self.nodes[index].dirty.paint = true;
        }
        Ok(())
    }
}
pub(crate) fn input_scroll(input: &TextInput, text: &ara_ui_text::Text, width: f32) -> f32 {
    let cursor = parley::Cursor::from_byte_index(
        &text.layout,
        input.display_caret(),
        parley::Affinity::Downstream,
    )
    .geometry(&text.layout, 1.0);
    (cursor.x0 as f32 - width + 2.0).max(0.0)
}
pub(crate) fn collect(nodes: &[crate::runtime::Node]) -> Result<BTreeMap<u64, Widget>> {
    let mut out = BTreeMap::new();
    for n in nodes {
        if let Some(w) = Widget::parse(&n.element)? {
            out.insert(stable_id(&n.element.id), w);
        }
    }
    Ok(out)
}
