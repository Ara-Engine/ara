//! Retained tree, independent invalidation lanes, Taffy measurement and display-list emission.
use crate::{
    Result,
    paint::{Box2, DisplayList},
    source::{self, Element, stable_id},
    style::{Computed, Sheet, State},
};
use ara_ui_text::Text;
use serde::Serialize;
use taffy::prelude::*;
/// Independent invalidation lanes; paint-only changes preserve layout and shaped text.
#[derive(Default)]
pub struct Dirty {
    /// Needs selector/cascade evaluation.
    pub style: bool,
    /// Needs text measurement or geometry recomputation.
    pub layout: bool,
    /// Needs display-list regeneration.
    pub paint: bool,
}
/// Retained node and cached layout/text state, indexed by its stable authored identity.
pub struct Node {
    /// Full virtual content extent; a row/card pool owns vertical translation.
    pub virtual_extent: Option<[f32; 2]>,
    /// Runtime paint translation, preserved across restyles and theme switches.
    pub paint_offset: [f32; 2],
    /// Runtime visibility, independent of authored style and theme switching.
    pub visible: bool,
    /// Compiled authoring metadata and ordered children.
    pub element: Element,
    /// Resolved native properties.
    pub computed: Computed,
    /// Current independent selector states.
    pub state: State,
    /// Opaque node in the retained Taffy tree.
    pub layout_id: NodeId,
    /// Cached paragraph, when this control displays text.
    pub text: Option<Text>,
    /// Separate incremental work flags.
    pub dirty: Dirty,
    /// Computed logical viewport rectangle.
    pub rect: Box2,
    /// Intersected ancestor clip bounds in viewport coordinates.
    pub clip: Box2,
}
/// One retained UI surface with layout, text caches, input states and an engine-neutral display list.
pub struct Ui {
    pub(crate) pointer: Option<[f32; 2]>,
    /// Native control state, indexed by stable identity.
    pub widgets: std::collections::BTreeMap<u64, crate::widgets::Widget>,
    pub(crate) actions: Vec<crate::binding::Action>,
    pub(crate) stylesheet_source: String,
    /// Retained nodes; offscreen virtual-model rows have no nodes.
    pub nodes: Vec<Node>,
    /// Retained Taffy layout storage and internal caches.
    pub tree: TaffyTree<usize>,
    /// Compiled stylesheet and active theme tokens.
    pub sheet: Sheet,
    /// Cached painter-ordered display list.
    pub display: DisplayList,
    /// Logical viewport width and height.
    pub size: [f32; 2],
    /// Current visible traversal order for hit testing and keyboard navigation.
    pub paint_order: Vec<usize>,
    /// Display-list index where each `paint_order` node's commands begin, so hosts
    /// can splice paint-time decorations beneath later layers (menus, dialogs).
    pub paint_starts: Vec<usize>,
    /// Stable ID -> node index for `find`/`by_id`; rebuilt lazily when nodes change.
    index: std::cell::RefCell<std::collections::HashMap<u64, usize>>,
    /// `nodes.len()` when `index` was built; `usize::MAX` forces a rebuild.
    indexed: std::cell::Cell<usize>,
    /// Editor surfaces reject raw colors in inline styles (D18), on reconcile too.
    pub(crate) editor_checks: bool,
    /// What `reconcile_parts` remembers between calls (part markup, index moves).
    pub(crate) parts: crate::reconcile::Parts,
    /// Where `frame` last placed each popover, by stable identity; reapplied over the
    /// popover's CSS whenever it restyles.
    pub(crate) placed: std::collections::BTreeMap<u64, crate::popover::Applied>,
}
/// Actual incremental work counters for profiling and focused tests; `+=` sums
/// several frames (a host's per-frame budget).
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Work {
    /// Nodes whose cascade was reevaluated.
    pub restyled: usize,
    /// Calls into the Taffy layout pipeline.
    pub layout_passes: usize,
    /// Paragraphs reshaped by Parley; rebreaking is excluded.
    pub shaped: usize,
    /// Retained nodes visited during display-list rebuilding.
    pub paint_nodes: usize,
}
impl std::ops::AddAssign for Work {
    fn add_assign(&mut self, other: Self) {
        self.restyled += other.restyled;
        self.layout_passes += other.layout_passes;
        self.shaped += other.shaped;
        self.paint_nodes += other.paint_nodes;
    }
}
impl Ui {
    pub(crate) fn focus_candidate(&self, index: usize) -> bool {
        let n = &self.nodes[index];
        if is_focusable(n) {
            return self.shown(index);
        }
        if !is_control(n) || !self.shown(index) {
            return false;
        }
        let mut parent = n.element.parent;
        while let Some(i) = parent {
            if self.scroll_position(i).is_some() {
                return true;
            }
            parent = self.nodes[i].element.parent;
        }
        false
    }
    /// Parse and eagerly validate all native syntax before creating runtime state.
    pub fn parse(markup: &str, css: &str, size: [f32; 2]) -> Result<Self> {
        Self::with_tokens(markup, css, size, std::collections::BTreeMap::new())
    }
    /// Construct a retained tree with explicit theme tokens.
    pub fn with_tokens(
        markup: &str,
        css: &str,
        size: [f32; 2],
        tokens: std::collections::BTreeMap<String, String>,
    ) -> Result<Self> {
        if size.iter().any(|n| !n.is_finite() || *n <= 0.0) {
            return Err("ARA4040: invalid viewport".into());
        }
        let elements = source::compile(markup)?;
        let sheet = Sheet::with_tokens(css, tokens)?;
        sheet.validate_inline(markup)?;
        let mut tree = TaffyTree::new();
        let mut nodes = Vec::new();
        for (i, element) in elements.into_iter().enumerate() {
            nodes.push(build_node(&mut tree, &sheet, element, i)?);
        }
        for node in &nodes {
            let children: Vec<_> = node
                .element
                .children
                .iter()
                .map(|i| nodes[*i].layout_id)
                .collect();
            tree.set_children(node.layout_id, &children)?;
        }
        Ok(Self {
            pointer: None,
            widgets: crate::widgets::collect(&nodes)?,
            actions: Vec::new(),
            stylesheet_source: css.into(),
            nodes,
            tree,
            sheet,
            display: DisplayList::default(),
            size,
            paint_order: Vec::new(),
            paint_starts: Vec::new(),
            index: Default::default(),
            indexed: std::cell::Cell::new(usize::MAX),
            editor_checks: false,
            parts: Default::default(),
            placed: Default::default(),
        })
    }
    /// Resolve an authored ID without panicking when the node is absent.
    pub fn find(&self, id: &str) -> Option<usize> {
        self.lookup(stable_id(id), |n| n.element.id == id)
    }
    /// Whether the node is laid out and painted as of the last frame: neither it nor an
    /// ancestor is hidden (`set_visible`, `display:none`). A hidden node keeps its
    /// subtree and state, but its geometry is stale.
    pub fn shown(&self, index: usize) -> bool {
        let mut at = Some(index);
        while let Some(i) = at {
            let Some(node) = self.nodes.get(i) else {
                return false;
            };
            if !node.visible || node.computed.layout.display == Display::None {
                return false;
            }
            if let Some(parent) = node.element.parent
                && !self.nodes[parent].element.children.contains(&i)
            {
                return false;
            }
            at = node.element.parent;
        }
        true
    }
    /// The node indexed under `key` that passes `is`. The index is rebuilt when nodes
    /// were added or a hit is stale (virtual rows rename their nodes in place).
    pub(crate) fn lookup(&self, key: u64, is: impl Fn(&Node) -> bool) -> Option<usize> {
        if self.indexed.get() == self.nodes.len() {
            match self.index.borrow().get(&key) {
                Some(&i) if self.nodes.get(i).is_some_and(&is) => return Some(i),
                None => return None,
                Some(_) => {}
            }
        }
        let mut index = self.index.borrow_mut();
        index.clear();
        for (i, n) in self.nodes.iter().enumerate() {
            index.entry(stable_id(&n.element.id)).or_insert(i);
        }
        self.indexed.set(self.nodes.len());
        index.get(&key).copied().filter(|i| is(&self.nodes[*i]))
    }
    /// Forget the ID index (after renaming nodes in place).
    pub(crate) fn invalidate_index(&self) {
        self.indexed.set(usize::MAX);
    }
    /// Push a changed display/model value, invalidating only the affected text and geometry.
    pub fn set_text(&mut self, index: usize, value: &str) -> Result<()> {
        let id = self.node_id(index)?;
        if let Some(crate::widgets::Widget::Input(input)) = self.widgets.get_mut(&id)
            && input.value() != value
        {
            *input = ara_ui_text::TextInput::new(value);
        }
        self.set_display_text(index, value)
    }
    pub(crate) fn set_display_text(&mut self, index: usize, value: &str) -> Result<()> {
        let node = self
            .nodes
            .get_mut(index)
            .ok_or("ARA4040: invalid node handle")?;
        let text = node.text.as_mut().ok_or("node is not a text control")?;
        if text.set(value) {
            node.dirty.layout = true;
            node.dirty.paint = true;
            self.tree.mark_dirty(node.layout_id)?;
        }
        Ok(())
    }
    /// Replace a node's inline style (its `style` attribute) in place: it restyles,
    /// and lays out again only if its box changed. Forget the part it belongs to
    /// ([`Self::forget_part`]) so the next reconcile does not keep it as stale.
    pub fn set_style(&mut self, index: usize, style: &str) -> Result<()> {
        let node = self
            .nodes
            .get_mut(index)
            .ok_or("ARA4040: invalid node handle")?;
        if node.element.attrs.get("style").map(String::as_str) != Some(style) {
            node.element.attrs.insert("style".into(), style.into());
            node.dirty.style = true;
        }
        Ok(())
    }
    /// Update selector states; unchanged states produce no work.
    pub fn set_state(&mut self, index: usize, state: State) {
        if self.nodes[index].state != state {
            self.nodes[index].state = state;
            self.nodes[index].dirty.style = true;
        }
    }
    /// Find the top visible, enabled control within transformed ancestor clips.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        if let Some((index, _)) = self.scrollbar_at([x, y]) {
            return Some(index);
        }
        self.paint_order.iter().copied().rev().find(|index| {
            let n = &self.nodes[*index];
            is_focusable(n)
                && n.rect.contains(x, y)
                && n.clip.contains(x, y)
                && self.rounded_hit(*index, x, y)
        })
    }
    pub(crate) fn rounded_hit(&self, index: usize, x: f32, y: f32) -> bool {
        let mut current = Some(index);
        while let Some(i) = current {
            let n = &self.nodes[i];
            if n.computed.clip && n.computed.radius > 0.0 {
                let r = n.computed.radius.min(n.rect.w / 2.0).min(n.rect.h / 2.0);
                let cx = x.clamp(n.rect.x + r, n.rect.x + n.rect.w - r);
                let cy = y.clamp(n.rect.y + r, n.rect.y + n.rect.h - r);
                if (x - cx).powi(2) + (y - cy).powi(2) > r * r {
                    return false;
                }
            }
            current = n.element.parent;
        }
        true
    }
    /// Return computed geometry, text coverage, provenance, focus graph and batch diagnostics.
    pub fn inspect(&self) -> serde_json::Value {
        let nodes:Vec<_>=self.nodes.iter().map(|n| {
            let mut warnings=Vec::new();
            if let Some(parent)=n.element.parent {let p=self.nodes[parent].rect;if n.rect.x+n.rect.w>p.x+p.w+0.5||n.rect.y+n.rect.h>p.y+p.h+0.5 {warnings.push("overflow-parent");}}
            let layout=self.tree.layout(n.layout_id).expect("retained nodes have layouts");
            if n.text.as_ref().is_some_and(|t|t.layout.height()>n.rect.h-layout.padding.top-layout.padding.bottom-layout.border.top-layout.border.bottom+0.5) {warnings.push("clipped-text-height");}
            if n.text.as_ref().is_some_and(|t|t.layout.width()>n.rect.w-layout.padding.left-layout.padding.right-layout.border.left-layout.border.right+0.5) {warnings.push("clipped-text-width");}
            serde_json::json!({"id":n.element.id,"stable_id":stable_id(&n.element.id),"box":n.rect,"clip":n.clip,"warnings":warnings,"text":n.text.as_ref().map(ara_ui_text::inspect),"provenance":self.sheet.provenance(&n.element,n.state)})
        }).collect();
        serde_json::json!({"viewport":self.size,"nodes":nodes,"focus_graph":crate::input::focus_graph(self),"batch_breaks":self.display.breaks(),"commands":self.display.commands.len(),"glyphs":self.display.glyphs.len()})
    }
}
/// A retained node for `element` at `index`, styled and with a Taffy leaf; its layout
/// and paint are dirty, and its style too when it inherits its parent's color (the
/// first frame resolves that, parents first).
pub(crate) fn build_node(
    tree: &mut TaffyTree<usize>,
    sheet: &Sheet,
    element: Element,
    index: usize,
) -> Result<Node> {
    let state = State {
        disabled: element.attrs.get("disabled").is_some_and(|v| v == "true"),
        ..State::default()
    };
    let computed = sheet.compute(&element, state)?;
    let text = element.attrs.get("text").map(|v| Text::new(v.to_owned()));
    let layout_id = tree.new_leaf_with_context(computed.layout.clone(), index)?;
    Ok(Node {
        virtual_extent: None,
        paint_offset: [0.0; 2],
        visible: true,
        element,
        dirty: Dirty {
            style: computed.inherit_color,
            layout: true,
            paint: true,
        },
        computed,
        state,
        layout_id,
        text,
        rect: Box2::default(),
        clip: Box2::default(),
    })
}
/// Native UI is focusable.
pub fn is_focusable(n: &Node) -> bool {
    is_control(n) && n.rect.intersects(n.clip)
}
fn is_control(n: &Node) -> bool {
    matches!(
        n.element.tag.as_str(),
        "button"
            | "field"
            | "input"
            | "toggle"
            | "slider"
            | "splitter"
            | "tabs"
            | "tab"
            | "dropdown"
            | "menu"
    ) && n.visible
        && !n.state.disabled
}
