//! Incremental updates from regenerated markup. A host may rebuild its markup on
//! every model change and call [`Ui::reconcile`]: nodes whose stable ID survives keep
//! their retained state (shaped text, Taffy node and cached layout, computed style,
//! hover/focus/press state, runtime visibility and paint offset, live widget state),
//! so only what changed is restyled, reshaped or laid out again.
//!
//! A host with large, independent regions (an editor's dock areas) can split its
//! markup into parts ([`Ui::reconcile_parts`]): a part whose markup is unchanged since
//! the last reconcile keeps its whole subtree, runtime-appended nodes (virtual rows)
//! included, without being compiled or matched again.
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
    /// Nodes of unchanged parts kept wholesale ([`Ui::reconcile_parts`]).
    pub moved: usize,
}

impl Reconciled {
    /// Whether the tree is exactly as before.
    pub fn unchanged(&self) -> bool {
        self.changed == 0 && self.added == 0 && self.removed == 0
    }
}

/// What [`Ui::reconcile_parts`] remembers between calls.
#[derive(Default)]
pub(crate) struct Parts {
    /// The last skeleton markup and its compiled elements.
    skeleton: String,
    skeleton_elements: Vec<Element>,
    /// Host ID -> the part's markup at the last reconcile.
    markup: HashMap<String, String>,
    /// Old node index -> new index after the last reconcile (`usize::MAX`: dropped);
    /// None when indices did not change.
    remap: Option<Vec<usize>>,
}

/// Attributes that define a widget's initial state; while they are unchanged a kept
/// widget keeps its live state (caret, undo, open popup, slider value, scroll).
const WIDGET_ATTRS: &[&str] = &["text", "checked", "min", "max", "step", "value", "selected"];
/// Attributes the cascade reads (with the tag, which a kept node shares).
const STYLE_ATTRS: &[&str] = &["class", "style", "disabled"];

/// One node of the reconciled tree, in its new (preorder) position.
enum Source {
    /// A compiled element, children and parent in new indices; matched to an old
    /// node by ID and tag.
    New(Element),
    /// An old node kept wholesale (an unchanged part); `root` is the new parent of a
    /// part's root.
    Old {
        index: usize,
        root: Option<Option<usize>>,
    },
}

impl Ui {
    /// Update the retained tree to `markup` (same stylesheet and theme). Nodes match by
    /// ID and tag; see the module docs for what a kept node retains. A kept node's text
    /// follows the markup, except a kept input: it keeps its live edit (caret,
    /// selection, undo) while its `text` attribute is unchanged or equals its value
    /// (a host echoing each keystroke). Runtime-appended children (virtual rows) are
    /// dropped unless the markup lists them.
    pub fn reconcile(&mut self, markup: &str) -> Result<Reconciled> {
        let elements = source::compile(markup)?;
        let sources = elements.into_iter().map(Source::New).collect();
        let report = self.apply(sources, &[])?;
        // Indices start over: nothing was kept wholesale.
        self.parts = Parts {
            remap: Some(Vec::new()),
            ..Parts::default()
        };
        Ok(report)
    }

    /// As [`reconcile`](Self::reconcile), for markup split into a `skeleton` and
    /// `parts` (host ID, part markup): each part's root element has its host's ID and
    /// replaces the skeleton element with that ID. A part whose markup is the same as
    /// at the last call keeps its subtree exactly as it is, runtime state and
    /// runtime-appended nodes included; nodes only change index (see
    /// [`moved_index`](Self::moved_index)). Nothing is touched when the skeleton and
    /// every part are unchanged. A rejected call leaves the tree intact.
    pub fn reconcile_parts(
        &mut self,
        skeleton: &str,
        parts: &[(&str, &str)],
    ) -> Result<Reconciled> {
        let mut cache = std::mem::take(&mut self.parts);
        let result = self.splice(&mut cache, skeleton, parts);
        self.parts = if result.is_ok() {
            cache
        } else {
            Parts::default()
        };
        result
    }

    /// Where the node at `old` before the last reconcile is now, if it was kept
    /// wholesale (or nothing changed); `VirtualView::remap` follows its rows with it.
    pub fn moved_index(&self, old: usize) -> Option<usize> {
        match &self.parts.remap {
            None => (old < self.nodes.len()).then_some(old),
            Some(remap) => remap.get(old).copied().filter(|i| *i != usize::MAX),
        }
    }

    fn splice(
        &mut self,
        cache: &mut Parts,
        skeleton: &str,
        parts: &[(&str, &str)],
    ) -> Result<Reconciled> {
        let same_skeleton = cache.skeleton == skeleton && !cache.skeleton_elements.is_empty();
        let mut kept = Vec::new();
        let mut fresh = HashMap::new();
        for (host, markup) in parts {
            if cache.markup.get(*host).is_some_and(|m| m == markup)
                && let Some(root) = self.find(host)
            {
                kept.push((*host, root));
            } else {
                let elements = source::compile(markup)?;
                if elements.first().is_none_or(|e| e.id != *host) {
                    return Err(format!(
                        "ARA4042: part `{host}` must have its host's ID at its root"
                    )
                    .into());
                }
                fresh.insert(*host, elements);
            }
        }
        if same_skeleton && fresh.is_empty() && cache.markup.len() == parts.len() {
            cache.remap = None;
            return Ok(Reconciled {
                kept: self.nodes.len(),
                ..Default::default()
            });
        }
        if !same_skeleton {
            cache.skeleton_elements = source::compile(skeleton)?;
            skeleton.clone_into(&mut cache.skeleton);
        }
        // Each old node's kept part, by its parent chain: parents precede children,
        // and virtual rows scrolled out of view are detached from their parent's
        // children but still belong to it.
        let roots: HashMap<usize, usize> = kept
            .iter()
            .enumerate()
            .map(|(k, (_, root))| (*root, k))
            .collect();
        let mut owner: Vec<Option<usize>> = vec![None; self.nodes.len()];
        let mut members = vec![Vec::new(); kept.len()];
        for i in 0..self.nodes.len() {
            let part = roots.get(&i).copied().or_else(|| {
                self.nodes[i]
                    .element
                    .parent
                    .filter(|p| *p < i)
                    .and_then(|p| owner[p])
            });
            owner[i] = part;
            if let Some(k) = part {
                members[k].push(i);
            }
        }
        let mut assembly = Assembly {
            skeleton: &cache.skeleton_elements,
            kept: kept
                .iter()
                .enumerate()
                .map(|(k, (host, _))| (*host, k))
                .collect(),
            members,
            fresh,
            sources: Vec::with_capacity(self.nodes.len()),
            remap: vec![usize::MAX; self.nodes.len()],
            placed: 0,
        };
        if !cache.skeleton_elements.is_empty() {
            assembly.skeleton_node(0, None);
        }
        if assembly.placed != parts.len() {
            return Err("ARA4042: every part needs a host element in the skeleton".into());
        }
        let Assembly { sources, remap, .. } = assembly;
        let report = self.apply(sources, &remap)?;
        cache.markup = parts
            .iter()
            .map(|(h, m)| ((*h).to_owned(), (*m).to_owned()))
            .collect();
        cache.remap = Some(remap);
        Ok(report)
    }

    /// Replace the tree with `sources` (see [`Source`]); `remap` maps old indices of
    /// wholesale-kept nodes to their new ones.
    fn apply(&mut self, sources: Vec<Source>, remap: &[usize]) -> Result<Reconciled> {
        // New elements match only nodes that are not kept wholesale.
        let mut by_id: HashMap<u64, usize> = HashMap::with_capacity(self.nodes.len());
        for (i, node) in self.nodes.iter().enumerate() {
            if remap.get(i).is_none_or(|n| *n == usize::MAX) {
                by_id.entry(stable_id(&node.element.id)).or_insert(i);
            }
        }
        self.validate(
            sources.iter().filter_map(|s| match s {
                Source::New(e) => Some(e),
                Source::Old { .. } => None,
            }),
            &by_id,
        )?;
        // Nothing below fails on its own: the tree is replaced, or left as it was.
        let mut old: Vec<Option<Node>> = std::mem::take(&mut self.nodes)
            .into_iter()
            .map(Some)
            .collect();
        let mut old_widgets = std::mem::take(&mut self.widgets);
        let mut widgets = BTreeMap::new();
        let mut report = Reconciled::default();
        let mut nodes = Vec::with_capacity(sources.len());
        let mut matched = Vec::new();
        // A node changed index: the cached paint order is stale.
        let mut shifted = false;
        for (index, source) in sources.into_iter().enumerate() {
            let node = match source {
                Source::New(element) => {
                    matched.push(index);
                    let id = stable_id(&element.id);
                    let kept = by_id.get(&id).and_then(|j| {
                        shifted |= *j != index;
                        old[*j]
                            .take_if(|n| n.element.tag == element.tag && n.element.id == element.id)
                    });
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
                    node
                }
                Source::Old { index: from, root } => {
                    let mut node = old[from]
                        .take()
                        .ok_or("ARA4042: a kept node was claimed twice")?;
                    node.element.parent = match root {
                        Some(parent) => parent,
                        None => node.element.parent.map(|p| remap[p]),
                    };
                    for child in &mut node.element.children {
                        *child = remap[*child];
                    }
                    if let Some(widget) = old_widgets.remove(&stable_id(&node.element.id)) {
                        widgets.insert(stable_id(&node.element.id), widget);
                    }
                    if from != index {
                        shifted = true;
                        self.tree.set_node_context(node.layout_id, Some(index))?;
                    }
                    report.moved += 1;
                    node
                }
            };
            nodes.push(node);
        }
        // Dropped nodes leave first, so their kept children are free to move.
        for node in old.into_iter().flatten() {
            self.tree.remove(node.layout_id)?;
            report.removed += 1;
        }
        // Wholesale-kept subtrees keep their Taffy children; matched nodes may differ.
        let wanted: Vec<(usize, Vec<taffy::NodeId>)> = matched
            .into_iter()
            .map(|i| {
                let children = nodes[i]
                    .element
                    .children
                    .iter()
                    .map(|c| nodes[*c].layout_id)
                    .collect();
                (i, children)
            })
            .filter(|(i, children)| {
                self.tree
                    .children(nodes[*i].layout_id)
                    .is_ok_and(|current| current != *children)
            })
            .collect();
        // Clear every changed parent before re-parenting, so a child moving between
        // two of them never loses its new parent link.
        for (i, _) in &wanted {
            self.tree.set_children(nodes[*i].layout_id, &[])?;
        }
        for (i, children) in &wanted {
            self.tree.set_children(nodes[*i].layout_id, children)?;
            nodes[*i].dirty.layout = true;
        }
        // Any structural change lays out again: Taffy dropped removed nodes from their
        // parents' child lists itself (so those parents are not in `wanted`) and
        // recomputes only the subtrees it marked dirty. Shifted indices rebuild the
        // paint order.
        let structural = report.added > 0 || report.removed > 0 || !wanted.is_empty();
        if let Some(root) = nodes.first_mut() {
            root.dirty.layout |= structural;
            root.dirty.paint |= structural || shifted || report.changed > 0;
        }
        self.nodes = nodes;
        self.widgets = widgets;
        self.invalidate_index();
        Ok(report)
    }

    /// Everything a reconcile can reject, checked before the tree changes: widget
    /// values, and for new or restyled nodes their cascade and (editor surfaces, D18)
    /// raw colors in inline styles.
    fn validate<'a>(
        &self,
        elements: impl Iterator<Item = &'a Element>,
        by_id: &HashMap<u64, usize>,
    ) -> Result<()> {
        for element in elements {
            let old = by_id
                .get(&stable_id(&element.id))
                .map(|i| &self.nodes[*i])
                .filter(|n| n.element.tag == element.tag && n.element.id == element.id);
            let restyled = old.is_none_or(|n| {
                STYLE_ATTRS
                    .iter()
                    .any(|a| n.element.attrs.get(*a) != element.attrs.get(*a))
            });
            if restyled {
                if self.editor_checks {
                    crate::theme::check_node_style(element)?;
                }
                let state = crate::style::State {
                    disabled: element.attrs.get("disabled").is_some_and(|v| v == "true"),
                    ..Default::default()
                };
                self.sheet.compute(element, state)?;
            }
            Widget::parse(element)?;
        }
        Ok(())
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

/// Lays out the new tree's sources: skeleton elements, compiled parts and kept parts
/// in preorder.
struct Assembly<'a> {
    skeleton: &'a [Element],
    /// Host ID -> kept part ordinal (an index into `members`).
    kept: HashMap<&'a str, usize>,
    /// Each kept part's old node indices, root first, parents before children.
    members: Vec<Vec<usize>>,
    /// Host ID -> a changed part's compiled elements.
    fresh: HashMap<&'a str, Vec<Element>>,
    sources: Vec<Source>,
    remap: Vec<usize>,
    /// Parts placed so far (each must have a host).
    placed: usize,
}

impl Assembly<'_> {
    /// Emit skeleton element `i` (or the part it hosts) under `parent`; returns its
    /// new index.
    fn skeleton_node(&mut self, i: usize, parent: Option<usize>) -> usize {
        let id = self.skeleton[i].id.as_str();
        if let Some(k) = self.kept.get(id).copied() {
            self.placed += 1;
            let first = self.sources.len();
            for old in std::mem::take(&mut self.members[k]) {
                self.remap[old] = self.sources.len();
                self.sources.push(Source::Old {
                    index: old,
                    root: None,
                });
            }
            if let Some(Source::Old { root, .. }) = self.sources.get_mut(first) {
                *root = Some(parent);
            }
            return first;
        }
        if let Some(elements) = self.fresh.remove(id) {
            self.placed += 1;
            let offset = self.sources.len();
            for (j, mut element) in elements.into_iter().enumerate() {
                element.parent = if j == 0 {
                    parent
                } else {
                    element.parent.map(|p| p + offset)
                };
                for child in &mut element.children {
                    *child += offset;
                }
                self.sources.push(Source::New(element));
            }
            return offset;
        }
        let index = self.sources.len();
        let mut element = self.skeleton[i].clone();
        element.children.clear();
        element.parent = parent;
        self.sources.push(Source::New(element));
        for child in self.skeleton[i].children.clone() {
            let placed = self.skeleton_node(child, Some(index));
            if let Source::New(element) = &mut self.sources[index] {
                element.children.push(placed);
            }
        }
        index
    }
}
