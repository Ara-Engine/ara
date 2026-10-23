//! Key-preserving bounded row pools. Scrolling visits visible rows, never the full model.
use crate::{
    Result, Ui,
    source::{Element, escape, stable_id},
    style::State,
};
use std::collections::{BTreeMap, BTreeSet};
/// Borrowed model row; keys must be stable and unique within the collection.
pub struct Row<'a> {
    /// Model identity, independent of index.
    pub key: u64,
    /// Indentation depth, for hierarchy controls.
    pub depth: u32,
    /// Text for each column.
    pub cells: Vec<&'a str>,
}
/// Event-driven collection seam; no hidden retained nodes for offscreen data.
pub trait Rows {
    /// Current visible-model row count.
    fn len(&self) -> usize;
    /// Borrow one row in logical order.
    fn row(&self, index: usize) -> Row<'_>;
    /// Whether the projection is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
/// Stable keyed list model, with UI storage bounded by the viewport.
pub struct VirtualList {
    /// Logical item order and text, updated by the host's collection notifications.
    pub items: Vec<(u64, String)>,
}
impl Rows for VirtualList {
    fn len(&self) -> usize {
        self.items.len()
    }
    fn row(&self, index: usize) -> Row<'_> {
        let (key, text) = &self.items[index];
        Row {
            key: *key,
            depth: 0,
            cells: vec![text],
        }
    }
}
/// Input tree node, stored once per model node rather than as a UI control.
pub struct TreeItem {
    /// Stable model key.
    pub key: u64,
    /// Parent index, which must precede this item.
    pub parent: Option<usize>,
    /// Display name.
    pub text: String,
    /// Whether children are visible.
    pub expanded: bool,
}
/// Flat expanded projection; changes rebuild once on notification, scrolls are O(visible).
pub struct VirtualTree {
    items: Vec<TreeItem>,
    visible: Vec<usize>,
    depth: Vec<u32>,
}
impl VirtualTree {
    /// Validate a parent-before-child preorder tree and build its expanded projection.
    pub fn new(items: Vec<TreeItem>) -> Result<Self> {
        let mut keys = BTreeSet::new();
        let mut depth = Vec::with_capacity(items.len());
        for (i, n) in items.iter().enumerate() {
            if !keys.insert(n.key) || n.parent.is_some_and(|p| p >= i) {
                return Err("ARA7041: duplicate tree key or invalid parent order".into());
            }
            depth.push(n.parent.map_or(0, |p| depth[p] + 1));
        }
        let mut tree = Self {
            items,
            visible: Vec::new(),
            depth,
        };
        tree.rebuild();
        Ok(tree)
    }
    fn rebuild(&mut self) {
        self.visible.clear();
        let mut shown = vec![false; self.items.len()];
        for (i, n) in self.items.iter().enumerate() {
            let visible = n.parent.is_none_or(|p| shown[p] && self.items[p].expanded);
            shown[i] = visible;
            if visible {
                self.visible.push(i);
            }
        }
    }
    /// Expand/collapse by stable model key, called only on model changes or input.
    pub fn set_expanded(&mut self, key: u64, expanded: bool) -> Result<()> {
        let n = self
            .items
            .iter_mut()
            .find(|n| n.key == key)
            .ok_or("ARA7041: unknown tree key")?;
        if n.expanded != expanded {
            n.expanded = expanded;
            self.rebuild();
        }
        Ok(())
    }
    /// Total source nodes, including collapsed descendants.
    pub fn total_nodes(&self) -> usize {
        self.items.len()
    }
}
impl Rows for VirtualTree {
    fn len(&self) -> usize {
        self.visible.len()
    }
    fn row(&self, index: usize) -> Row<'_> {
        let i = self.visible[index];
        Row {
            key: self.items[i].key,
            depth: self.depth[i],
            cells: vec![&self.items[i].text],
        }
    }
}
/// A table projection supplies real independently laid-out columns.
pub struct VirtualTable {
    /// Stable row keys and column values.
    pub rows: Vec<(u64, Vec<String>)>,
    /// Pixel widths; the host may resize these on splitter input.
    pub widths: Vec<f32>,
}
impl Rows for VirtualTable {
    fn len(&self) -> usize {
        self.rows.len()
    }
    fn row(&self, index: usize) -> Row<'_> {
        let (key, cells) = &self.rows[index];
        Row {
            key: *key,
            depth: 0,
            cells: cells.iter().map(String::as_str).collect(),
        }
    }
}
struct Slot {
    key: Option<u64>,
    node: usize,
    cells: Vec<usize>,
}
/// Virtualized list/tree/table mounted into a retained scroll node.
pub struct VirtualView {
    parent: usize,
    namespace: String,
    slots: Vec<Slot>,
    order: Vec<usize>,
    row_height: f32,
    columns: Vec<f32>,
    offset: f32,
    first: usize,
}
impl VirtualView {
    /// Allocate only enough rows for the viewport plus two fractional-scroll guard rows.
    pub fn mount(
        ui: &mut Ui,
        parent: usize,
        row_height: f32,
        viewport: f32,
        columns: Vec<f32>,
    ) -> Result<Self> {
        if row_height <= 0.0
            || !row_height.is_finite()
            || viewport < 0.0
            || !viewport.is_finite()
            || columns.is_empty()
            || columns.iter().any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("ARA7041: invalid virtual viewport/columns".into());
        }
        let namespace = ui
            .nodes
            .get(parent)
            .ok_or("ARA7041: virtual parent not found")?
            .element
            .id
            .clone();
        if !ui.nodes[parent].element.children.is_empty() {
            return Err("ARA7041: mount requires an empty scroll node".into());
        }
        let count = (viewport / row_height).ceil() as usize + 2;
        let mut slots = Vec::with_capacity(count);
        for i in 0..count {
            let id = format!("{namespace}/pool-{i}");
            let row = ui.append(
                parent,
                Element {
                    id: id.clone(),
                    tag: "button".into(),
                    attrs: BTreeMap::from([
                        ("id".into(), id),
                        ("class".into(), "virtual-row".into()),
                        ("action".into(), "ActivateRow".into()),
                        (
                            "style".into(),
                            format!(
                                "height:{row_height}px; flex-direction:row; gap:0px; padding:0px"
                            ),
                        ),
                    ]),
                    children: Vec::new(),
                    parent: Some(parent),
                },
            )?;
            let mut cells = Vec::new();
            for (col, width) in columns.iter().enumerate() {
                let id = format!("{namespace}/pool-{i}/{col}");
                cells.push(ui.append(row,Element{id:id.clone(),tag:"text".into(),attrs:BTreeMap::from([("id".into(),id),("text".into(),String::new()),("style".into(),format!("width:{width}px; height:{row_height}px; padding:3px; overflow:hidden; font-size:13px"))]),children:Vec::new(),parent:Some(row)})?);
            }
            slots.push(Slot {
                key: None,
                node: row,
                cells,
            });
        }
        Ok(Self {
            parent,
            namespace,
            slots,
            order: Vec::with_capacity(count),
            row_height,
            columns,
            offset: 0.0,
            first: usize::MAX,
        })
    }
    /// Push a scroll/model change. Surviving keys retain their text, interaction and node handles.
    pub fn scroll(
        &mut self,
        ui: &mut Ui,
        model: &impl Rows,
        offset: f32,
        viewport: f32,
    ) -> Result<()> {
        if !offset.is_finite() || !viewport.is_finite() {
            return Err("ARA7041: nonfinite scroll".into());
        }
        let offset = offset.clamp(
            0.0,
            (model.len() as f32 * self.row_height - viewport).max(0.0),
        );
        let first = (offset / self.row_height).floor() as usize;
        let count = self.slots.len().min(model.len().saturating_sub(first));
        let rows: Vec<_> = (first..first + count).map(|i| model.row(i)).collect();
        let mut wanted = BTreeSet::new();
        for r in &rows {
            if !wanted.insert(r.key) || r.cells.len() != self.columns.len() {
                return Err("ARA7041: duplicate visible key or wrong column count".into());
            }
        }
        let existing: BTreeMap<_, _> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.key.map(|k| (k, i)))
            .collect();
        let mut free: Vec<_> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.key.is_none_or(|k| !wanted.contains(&k)))
            .map(|(i, _)| i)
            .rev()
            .collect();
        self.order.clear();
        for row in rows {
            let slot = if let Some(i) = existing.get(&row.key) {
                *i
            } else {
                free.pop().ok_or("ARA7041: row pool exhausted")?
            };
            let s = &mut self.slots[slot];
            let entering = s.key != Some(row.key);
            if entering {
                s.key = Some(row.key);
                ui.nodes[s.node].element.id = format!("{}/key-{}", self.namespace, row.key);
                ui.nodes[s.node].state = State::default();
                ui.nodes[s.node].dirty.style = true;
                for (col, index) in s.cells.iter().enumerate() {
                    ui.nodes[*index].element.id =
                        format!("{}/key-{}/{col}", self.namespace, row.key);
                }
            }
            for (col, index) in s.cells.iter().enumerate() {
                ui.set_text(*index, row.cells[col])?;
            }
            let node = &mut ui.nodes[s.node];
            let style = format!(
                "height:{}px; flex-direction:row; gap:0px; padding:0px; padding-left:{}px",
                self.row_height,
                row.depth as f32 * 14.0
            );
            if node.element.attrs.get("style") != Some(&style) {
                node.element.attrs.insert("style".into(), style);
                node.dirty.style = true;
            }
            self.order.push(s.node);
        }
        for s in &mut self.slots {
            if s.key.is_some_and(|k| !wanted.contains(&k)) {
                s.key = None;
            }
        }
        ui.nodes[self.parent].element.children.clear();
        ui.nodes[self.parent]
            .element
            .children
            .extend_from_slice(&self.order);
        let children: Vec<_> = self.order.iter().map(|i| ui.nodes[*i].layout_id).collect();
        ui.tree
            .set_children(ui.nodes[self.parent].layout_id, &children)?;
        // Fractional scrolling is a paint translation; only entering items are shaped.
        let remainder = offset - first as f32 * self.row_height;
        for index in &self.order {
            ui.nodes[*index].paint_offset[1] = -remainder;
        }
        ui.nodes[self.parent].dirty.layout = true;
        self.offset = offset;
        self.first = first;
        Ok(())
    }
    /// Retained row handles in visual/tab order.
    pub fn visible_nodes(&self) -> &[usize] {
        &self.order
    }
    /// Stable identity for focus restoration, independent of pool reuse.
    pub fn identity(&self, key: u64) -> u64 {
        stable_id(&format!("{}/key-{key}", self.namespace))
    }
    /// Source markup helper for a standalone virtual viewport.
    pub fn markup(id: &str, height: f32) -> String {
        format!(
            "<scroll id=\"{}\" style=\"height:{height}px; overflow:hidden\" />",
            escape(id, b'"')
        )
    }
}
impl Ui {
    pub(crate) fn append(&mut self, parent: usize, element: Element) -> Result<usize> {
        let index = self.nodes.len();
        let computed = self.sheet.compute(&element, State::default())?;
        let layout_id = self
            .tree
            .new_leaf_with_context(computed.layout.clone(), index)?;
        let text = element
            .attrs
            .get("text")
            .map(|t| ara_ui_text::Text::new(t.clone()));
        self.nodes.push(crate::runtime::Node {
            paint_offset: [0.0; 2],
            visible: true,
            element,
            computed,
            state: State::default(),
            layout_id,
            text,
            dirty: crate::runtime::Dirty {
                style: false,
                layout: true,
                paint: true,
            },
            rect: Default::default(),
            clip: Default::default(),
        });
        self.nodes[parent].element.children.push(index);
        self.nodes[parent].dirty.layout = true;
        self.tree
            .add_child(self.nodes[parent].layout_id, layout_id)?;
        Ok(index)
    }
}
