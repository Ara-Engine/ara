//! Incremental updates from regenerated markup. A host may rebuild its markup on
//! every model change and call [`Ui::reconcile`]: nodes whose stable ID survives keep
//! their retained state (shaped text, Taffy node and cached layout, computed style,
//! hover/focus/press state, runtime visibility and paint offset, live widget state),
//! so only what changed is restyled, reshaped or laid out again.
use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::{
    Result, Ui,
    runtime::{Node, build_node},
    source::{self, Element, stable_id},
    widgets::Widget,
};

/// What [`Ui::reconcile`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Reconciled {
    /// Nodes kept as they were.
    pub kept: usize,
    /// Kept nodes whose attributes changed (restyled or given new text).
    pub changed: usize,
    /// Nodes created.
    pub added: usize,
    /// Nodes dropped.
    pub removed: usize,
}

impl Reconciled {
    /// Whether the tree is exactly as before.
    pub fn unchanged(&self) -> bool {
        self.changed == 0 && self.added == 0 && self.removed == 0
    }
}

/// Attributes that define a widget's initial state; while they are unchanged a kept
/// widget keeps its live state (caret, undo, open popup, slider value, scroll).
const WIDGET_ATTRS: &[&str] = &["text", "checked", "min", "max", "step", "value", "selected"];
/// Attributes the cascade reads (with the tag, which a kept node shares).
const STYLE_ATTRS: &[&str] = &["class", "style", "disabled"];

impl Ui {
    /// Update the retained tree to `markup` (same stylesheet and theme). Nodes match by
    /// ID and tag; see the module docs for what a kept node retains. A kept node's text
    /// follows the markup, except a kept input: it keeps its live edit (caret,
    /// selection, undo) while its `text` attribute is unchanged or equals its value
    /// (a host echoing each keystroke). Runtime-appended children (virtual rows) are
    /// dropped unless the markup lists them.
    pub fn reconcile(&mut self, markup: &str) -> Result<Reconciled> {
        if self.editor_checks {
            crate::theme::check_inline(markup)?;
        }
        let elements = source::compile(markup)?;
        let mut old: Vec<Option<Node>> = std::mem::take(&mut self.nodes)
            .into_iter()
            .map(Some)
            .collect();
        let mut by_id: HashMap<String, usize> = HashMap::with_capacity(old.len());
        for (i, node) in old.iter().enumerate() {
            if let Some(node) = node {
                by_id.entry(node.element.id.clone()).or_insert(i);
            }
        }
        let mut old_widgets = std::mem::take(&mut self.widgets);
        let mut widgets = BTreeMap::new();
        let mut report = Reconciled::default();
        let mut nodes = Vec::with_capacity(elements.len());
        for (index, element) in elements.into_iter().enumerate() {
            let id = stable_id(&element.id);
            let kept = by_id
                .get(&element.id)
                .and_then(|j| old[*j].take_if(|n| n.element.tag == element.tag));
            let node = match kept {
                Some(node) => {
                    let widget = old_widgets.remove(&id);
                    self.update(node, element, widget, &mut widgets, &mut report)?
                }
                None => {
                    report.added += 1;
                    if let Some(widget) = Widget::parse(&element)? {
                        widgets.insert(id, widget);
                    }
                    build_node(&mut self.tree, &self.sheet, element, index)?
                }
            };
            self.tree.set_node_context(node.layout_id, Some(index))?;
            nodes.push(node);
        }
        // Dropped nodes leave first, so their kept children are free to move.
        for node in old.into_iter().flatten() {
            self.tree.remove(node.layout_id)?;
            report.removed += 1;
        }
        let wanted: Vec<Vec<taffy::NodeId>> = nodes
            .iter()
            .map(|n| {
                n.element
                    .children
                    .iter()
                    .map(|c| nodes[*c].layout_id)
                    .collect()
            })
            .collect();
        let moved: Vec<usize> = (0..nodes.len())
            .filter(|i| {
                self.tree
                    .children(nodes[*i].layout_id)
                    .is_ok_and(|current| current != wanted[*i])
            })
            .collect();
        // Clear every changed parent before re-parenting, so a child moving between
        // two of them never loses its new parent link.
        for i in &moved {
            self.tree.set_children(nodes[*i].layout_id, &[])?;
        }
        for i in &moved {
            self.tree.set_children(nodes[*i].layout_id, &wanted[*i])?;
            nodes[*i].dirty.layout = true;
        }
        // Any structural change lays out again: Taffy dropped removed nodes from their
        // parents' child lists itself (so those parents are not in `moved`) and
        // recomputes only the subtrees it marked dirty.
        let structural = report.added > 0 || report.removed > 0 || !moved.is_empty();
        if let Some(root) = nodes.first_mut() {
            root.dirty.layout |= structural;
            root.dirty.paint |= structural || report.changed > 0;
        }
        self.nodes = nodes;
        self.widgets = widgets;
        self.invalidate_index();
        Ok(report)
    }

    /// Bring a kept node up to date with its new element.
    fn update(
        &mut self,
        mut node: Node,
        element: Element,
        widget: Option<Widget>,
        widgets: &mut BTreeMap<u64, Widget>,
        report: &mut Reconciled,
    ) -> Result<Node> {
        let same = |names: &[&str]| {
            names
                .iter()
                .all(|a| node.element.attrs.get(*a) == element.attrs.get(*a))
        };
        if node.element.attrs == element.attrs {
            report.kept += 1;
        } else {
            report.changed += 1;
            if !same(STYLE_ATTRS) {
                node.dirty.style = true;
            }
            let disabled = element.attrs.get("disabled").is_some_and(|v| v == "true");
            if node.state.disabled != disabled {
                node.state.disabled = disabled;
                node.dirty.style = true;
            }
        }
        // A host echoing an edited field's value back into its markup keeps the live
        // input (caret, selection, undo): its value already is the new text.
        let echoed = matches!(&widget, Some(Widget::Input(input))
            if element.attrs.get("text").map(String::as_str) == Some(input.value()));
        let keep = widget.is_some() && (same(WIDGET_ATTRS) || echoed);
        let widget = if keep {
            widget
        } else {
            Widget::parse(&element)?
        };
        let live_input = keep && matches!(widget, Some(Widget::Input(_)));
        let mut relayout = false;
        match (element.attrs.get("text"), &mut node.text) {
            (Some(value), Some(text)) => relayout = !live_input && text.set(value),
            (Some(value), None) => {
                node.text = Some(ara_ui_text::Text::new(value.clone()));
                relayout = true;
            }
            (None, Some(_)) => {
                node.text = None;
                relayout = true;
            }
            (None, None) => {}
        }
        if relayout {
            node.dirty.layout = true;
            node.dirty.paint = true;
            self.tree.mark_dirty(node.layout_id)?;
        }
        if let Some(widget) = widget {
            widgets.insert(stable_id(&element.id), widget);
        }
        node.element = element;
        Ok(node)
    }

    /// Replace the stylesheet (the theme's tokens stay); unchanged source is a no-op.
    /// Returns whether it changed, in which case every node restyles on the next frame.
    pub fn set_stylesheet(&mut self, css: &str) -> Result<bool> {
        if css == self.stylesheet_source {
            return Ok(false);
        }
        if self.editor_checks {
            let syntax = source::stylesheet::Stylesheet::parse(css)?;
            syntax.check_editor(css)?;
        }
        self.sheet = crate::style::Sheet::with_tokens(css, self.sheet.overrides().clone())?;
        css.clone_into(&mut self.stylesheet_source);
        for node in &mut self.nodes {
            node.dirty.style = true;
        }
        Ok(true)
    }
}
