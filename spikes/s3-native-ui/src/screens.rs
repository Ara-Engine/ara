//! Small fixtures generated through the same markup parser used for authored screens.
use crate::{runtime::Ui, source::Result, style::State};
use std::fmt::Write;
pub const CSS: &str = include_str!("../assets/theme.css");

pub struct SchemaField {
    pub name: &'static str,
    pub label: &'static str,
    pub value: &'static str,
    pub docs: &'static str,
}
pub const SCHEMA: &[SchemaField] = &[
    SchemaField {
        name: "position",
        label: "Position / m",
        value: "12.0, 0.0, −3.5",
        docs: "Local position relative to the parent.",
    },
    SchemaField {
        name: "rotation",
        label: "Rotation / deg",
        value: "0.0, 90.0, 0.0",
        docs: "Euler angles in authoring space.",
    },
    SchemaField {
        name: "scale",
        label: "Scale",
        value: "1.0, 1.0, 1.0",
        docs: "Local nonuniform scale.",
    },
    SchemaField {
        name: "health-value",
        label: "Health / 0–100",
        value: "86",
        docs: "Current vitality, replicated by the simulation.",
    },
    SchemaField {
        name: "mass",
        label: "Mass / kg",
        value: "4.25",
        docs: "Body mass used by physics.",
    },
];
pub fn inspector() -> String {
    let mut source = String::from(
        "<panel id='inspector' class='screen'><text id='inspector-kicker' class='eyebrow' text='ARA / INSPECTOR' /><text id='inspector-title' class='title' text='Navigation core' /><text id='inspector-subtitle' class='subtitle' text='Entity #core-07 · Prefab override' /><panel id='fields' class='section'><text id='fields-title' class='section-title' text='Transform &amp; state' />",
    );
    for field in SCHEMA {
        write!(&mut source,"<panel id='row-{}' class='row'><text id='label-{}' class='label' text='{}' /><field id='{}' label='{}' text='{}' /></panel>",field.name,field.name,field.label,field.name,field.label,field.value).expect("writing a String cannot fail");
    }
    source.push_str("</panel><button id='reset' text='Reset selected overrides' /><text id='provenance' class='subtitle' text='Values: local override / prefab / default' /></panel>");
    source
}
pub const VISIBLE_ROWS: usize = 12;
pub struct Item {
    pub key: usize,
    pub quantity: u32,
}
pub struct Inventory {
    pub items: Vec<Item>,
    pub ui: Ui,
    pub first: usize,
    pub row_indices: Vec<usize>,
    keys: Vec<usize>,
    layout_order: Vec<taffy::NodeId>,
    viewport: usize,
}
impl Inventory {
    pub fn new() -> Result<Self> {
        let items = (0..10_000)
            .map(|key| Item {
                key,
                quantity: (key % 19 + 1) as u32,
            })
            .collect();
        let mut markup = String::from(
            "<panel id='inventory' class='screen'><text id='inventory-kicker' class='eyebrow' text='OUTPOST / CARGO' /><text id='inventory-title' class='title' text='Salvage inventory' /><text id='inventory-range' class='subtitle' text='10,000 items · 12 visible rows' /><panel id='inventory-viewport' class='viewport'>",
        );
        for key in 0..VISIBLE_ROWS {
            write!(&mut markup,"<button id='item-{key}' class='inventory-row' text='Item {key:05}   /   salvage alloy   × 1' />").expect("writing a String cannot fail");
        }
        markup.push_str("</panel><text id='inventory-footer' class='subtitle' text='Stable item keys · recycled visible controls' /></panel>");
        let ui = Ui::parse(&markup, CSS, [720.0, 780.0])?;
        let row_indices = (0..VISIBLE_ROWS)
            .map(|key| ui.index(&format!("item-{key}")))
            .collect();
        let viewport = ui.index("inventory-viewport");
        Ok(Self {
            items,
            ui,
            first: usize::MAX,
            row_indices,
            keys: vec![usize::MAX; VISIBLE_ROWS],
            layout_order: Vec::with_capacity(VISIBLE_ROWS),
            viewport,
        })
    }
    /// Scroll by whole rows. Only the twelve visible controls exist, even near item 10,000.
    pub fn scroll_to(&mut self, first: usize) -> Result<()> {
        let first = first.min(self.items.len() - VISIBLE_ROWS);
        if first == self.first {
            return Ok(());
        }
        if self.first != usize::MAX {
            if first > self.first && first - self.first < VISIBLE_ROWS {
                self.row_indices.rotate_left(first - self.first);
                self.keys.rotate_left(first - self.first);
            } else if first < self.first && self.first - first < VISIBLE_ROWS {
                self.row_indices.rotate_right(self.first - first);
                self.keys.rotate_right(self.first - first);
            }
        }
        self.first = first;
        for (row, index) in self.row_indices.iter().copied().enumerate() {
            let item = &self.items[first + row];
            if self.keys[row] == item.key {
                continue;
            }
            self.keys[row] = item.key;
            let id = &mut self.ui.nodes[index].element.id;
            id.clear();
            write!(id, "item-{}", item.key)?;
            self.ui.set_state(index, State::default());
            self.ui.set_text(
                index,
                &format!(
                    "Item {:05}   /   salvage alloy   × {}",
                    item.key, item.quantity
                ),
            )?;
        }
        let viewport = &mut self.ui.nodes[self.viewport];
        viewport.element.children.clear();
        viewport
            .element
            .children
            .extend_from_slice(&self.row_indices);
        viewport.dirty.layout = true;
        self.layout_order.clear();
        self.layout_order
            .extend(self.row_indices.iter().map(|i| self.ui.nodes[*i].layout_id));
        self.ui
            .tree
            .set_children(self.ui.nodes[self.viewport].layout_id, &self.layout_order)?;
        Ok(())
    }
}

/// Produce actual AccessKit nodes rather than an approximation of its wire format.
pub fn accessibility(ui: &Ui, focus: &str) -> accesskit::TreeUpdate {
    use crate::source::stable_id;
    use accesskit::{Action, Node, NodeId, Rect, Role};
    let nodes = ui
        .nodes
        .iter()
        .map(|n| {
            let role = match n.element.tag.as_str() {
                "button" => Role::Button,
                "field" => Role::TextInput,
                "text" => Role::Label,
                _ => Role::GenericContainer,
            };
            let mut node = Node::new(role);
            node.set_children(
                n.element
                    .children
                    .iter()
                    .map(|i| NodeId(stable_id(&ui.nodes[*i].element.id)))
                    .collect::<Vec<_>>(),
            );
            node.set_label(
                n.element
                    .attrs
                    .get("label")
                    .map(String::as_str)
                    .unwrap_or_else(|| {
                        n.text
                            .as_ref()
                            .map(|t| t.value.as_str())
                            .unwrap_or(&n.element.id)
                    }),
            );
            if n.element.tag == "field" {
                if let Some(text) = &n.text {
                    node.set_value(text.value.as_str());
                }
                node.add_action(Action::SetValue);
            }
            if matches!(n.element.tag.as_str(), "field" | "button") {
                node.add_action(Action::Focus);
            }
            if n.element.tag == "button" {
                node.add_action(Action::Click);
            }
            if n.state.disabled {
                node.set_disabled();
            }
            if let Some(field) = SCHEMA.iter().find(|f| f.name == n.element.id) {
                node.set_description(field.docs);
            }
            node.set_bounds(Rect {
                x0: f64::from(n.rect.x),
                y0: f64::from(n.rect.y),
                x1: f64::from(n.rect.x + n.rect.w),
                y1: f64::from(n.rect.y + n.rect.h),
            });
            (NodeId(stable_id(&n.element.id)), node)
        })
        .collect();
    accesskit::TreeUpdate {
        nodes,
        tree: Some(accesskit::TreeInfo::new(NodeId(stable_id(
            &ui.nodes[0].element.id,
        )))),
        tree_id: accesskit::TreeId::ROOT,
        focus: NodeId(stable_id(focus)),
    }
}
