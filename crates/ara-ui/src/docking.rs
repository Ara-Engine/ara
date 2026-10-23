//! Serializable tab areas and split operations, independent of window/GPU ownership.
use crate::{Result, source::escape};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
/// Split orientation in logical UI pixels.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Axis {
    /// Side-by-side panels.
    Horizontal,
    /// Stacked panels.
    Vertical,
}
/// A serializable dock tree; panel IDs are owned by the editor registry.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum DockNode {
    /// Tabbed leaf area.
    Area {
        /// Stable area identity.
        id: String,
        /// Ordered panel IDs.
        tabs: Vec<String>,
        /// Active tab index, or zero for empty areas.
        active: usize,
    },
    /// Split with an adjustable fraction allocated to `first`.
    Split {
        /// Split orientation.
        axis: Axis,
        /// First child's fraction, constrained to 0.1–0.9.
        ratio: f32,
        /// First area/subtree.
        first: Box<DockNode>,
        /// Second area/subtree.
        second: Box<DockNode>,
    },
}
/// Drop target resolved from the dock overlay's pointer hit.
#[derive(Clone, Debug)]
pub enum DockDrop {
    /// Move to another area's tab bar (including reordering).
    Tab {
        /// Destination area.
        area: String,
        /// Insertion position.
        index: usize,
    },
    /// Split the destination area with the dragged tab.
    Split {
        /// Destination area.
        area: String,
        /// New area identity.
        new_area: String,
        /// Split axis.
        axis: Axis,
        /// Place the dragged area first.
        before: bool,
    },
}
/// Complete persisted layout with a validated reset baseline.
pub struct DockLayout {
    root: DockNode,
    default: DockNode,
}
impl DockLayout {
    /// Validate initial IDs/tabs/ratios before accepting a layout.
    pub fn new(root: DockNode) -> Result<Self> {
        validate(&root)?;
        Ok(Self {
            default: root.clone(),
            root,
        })
    }
    /// Current serializable layout.
    pub fn root(&self) -> &DockNode {
        &self.root
    }
    /// Update a persisted split after dragging its runtime splitter; path entries choose first/second.
    pub fn resize_split(&mut self, path: &[bool], ratio: f32) -> Result<()> {
        if !ratio.is_finite() {
            return Err("ARA7042: invalid split ratio".into());
        }
        let mut node = &mut self.root;
        for second in path {
            let DockNode::Split {
                first,
                second: other,
                ..
            } = node
            else {
                return Err("ARA7042: split path crosses a tab area".into());
            };
            node = if *second { other } else { first };
        }
        let DockNode::Split { ratio: value, .. } = node else {
            return Err("ARA7042: resize target is not a split".into());
        };
        *value = ratio.clamp(0.1, 0.9);
        Ok(())
    }
    /// Restore the registered default layout.
    pub fn reset(&mut self) {
        self.root = self.default.clone();
    }
    /// Stable JSON serialization; field order follows the typed enum schema.
    pub fn serialize(&self) -> Result<String> {
        Ok(ara_json::to_canonical_string(&ara_json::to_value(
            &&self.root,
        )?))
    }
    /// Atomically load a saved layout, retaining the original reset baseline.
    pub fn restore(&mut self, json: &str) -> Result<()> {
        let root = serde_json::from_value(ara_json::parse(json)?)?;
        validate(&root)?;
        self.root = root;
        Ok(())
    }
    /// Select a panel by stable identity.
    pub fn activate(&mut self, panel: &str) -> Result<()> {
        fn visit(n: &mut DockNode, p: &str) -> bool {
            match n {
                DockNode::Area { tabs, active, .. } => {
                    if let Some(i) = tabs.iter().position(|t| t == p) {
                        *active = i;
                        true
                    } else {
                        false
                    }
                }
                DockNode::Split { first, second, .. } => visit(first, p) || visit(second, p),
            }
        }
        if !visit(&mut self.root, panel) {
            return Err("ARA7042: unknown dock panel".into());
        }
        Ok(())
    }
    /// Drop a tab into another area or onto a split edge; failures leave the layout intact.
    pub fn drop_tab(&mut self, panel: &str, target: DockDrop) -> Result<()> {
        let mut candidate = self.root.clone();
        fn remove(n: &mut DockNode, p: &str) -> bool {
            match n {
                DockNode::Area { tabs, active, .. } => {
                    if let Some(i) = tabs.iter().position(|t| t == p) {
                        tabs.remove(i);
                        *active = (*active).min(tabs.len().saturating_sub(1));
                        true
                    } else {
                        false
                    }
                }
                DockNode::Split { first, second, .. } => remove(first, p) || remove(second, p),
            }
        }
        if !remove(&mut candidate, panel) {
            return Err("ARA7042: dragged panel not found".into());
        }
        let area = match &target {
            DockDrop::Tab { area, .. } | DockDrop::Split { area, .. } => area,
        };
        fn find<'a>(n: &'a mut DockNode, id: &str) -> Option<&'a mut DockNode> {
            match n {
                DockNode::Area { id: found, .. } if found == id => Some(n),
                DockNode::Split { first, second, .. } => {
                    find(first, id).or_else(|| find(second, id))
                }
                _ => None,
            }
        }
        let destination = find(&mut candidate, area).ok_or("ARA7042: dock destination missing")?;
        match target {
            DockDrop::Tab { index, .. } => {
                if let DockNode::Area { tabs, active, .. } = destination {
                    let i = index.min(tabs.len());
                    tabs.insert(i, panel.into());
                    *active = i;
                }
            }
            DockDrop::Split {
                new_area,
                axis,
                before,
                ..
            } => {
                let added = DockNode::Area {
                    id: new_area,
                    tabs: vec![panel.into()],
                    active: 0,
                };
                let old = destination.clone();
                let (first, second) = if before { (added, old) } else { (old, added) };
                *destination = DockNode::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
            }
        }
        validate(&candidate)?;
        self.root = candidate;
        Ok(())
    }
    /// Compile dock primitives into ordinary runtime markup; contents mount by panel ID.
    pub fn markup(&self) -> String {
        fn visit(n: &DockNode, out: &mut String, path: &str) {
            match n {
                DockNode::Area { id, tabs, active } => {
                    out.push_str(&format!("<panel id=\"dock-{}\" class=\"card\" style=\"flex-grow:1; min-width:100px\"><panel id=\"tabs-{}\" style=\"flex-direction:row; gap:4px\">",escape(id,b'"'),escape(id,b'"')));
                    for (i, tab) in tabs.iter().enumerate() {
                        out.push_str(&format!("<button id=\"tab-{}\" class=\"{}\" text=\"{}\" action=\"DockActivate\" />",escape(tab,b'"'),if i==*active{"selected"}else{""},escape(tab,b'"')));
                    }
                    out.push_str("</panel>");
                    if let Some(panel) = tabs.get(*active) {
                        out.push_str(&format!("<panel id=\"content-{}\" style=\"flex-grow:1\"><text id=\"label-{}\" class=\"muted\" text=\"{}\" /></panel>",escape(panel,b'"'),escape(panel,b'"'),escape(panel,b'"')));
                    }
                    out.push_str("</panel>");
                }
                DockNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => {
                    out.push_str(&format!("<panel id=\"split-{path}\" style=\"flex-grow:1; flex-direction:{}; gap:4px\"><panel id=\"first-{path}\" style=\"flex-grow:{}; flex-shrink:1; min-width:0px; min-height:0px\">",if *axis==Axis::Horizontal{"row"}else{"column"},ratio));
                    visit(first, out, &format!("{path}-a"));
                    out.push_str(&format!("</panel><splitter id=\"handle-{path}\" style=\"width:4px; height:4px\" /><panel id=\"second-{path}\" style=\"flex-grow:{}; flex-shrink:1; min-width:0px; min-height:0px\">",1.0-ratio));
                    visit(second, out, &format!("{path}-b"));
                    out.push_str("</panel></panel>");
                }
            }
        }
        let mut out = String::new();
        visit(&self.root, &mut out, "root");
        out
    }
}
fn validate(root: &DockNode) -> Result<()> {
    fn visit(
        n: &DockNode,
        areas: &mut BTreeSet<String>,
        panels: &mut BTreeSet<String>,
        depth: usize,
    ) -> bool {
        if depth > 64 {
            return false;
        }
        match n {
            DockNode::Area { id, tabs, active } => {
                !id.is_empty()
                    && areas.insert(id.clone())
                    && (tabs.is_empty() && *active == 0 || *active < tabs.len())
                    && tabs
                        .iter()
                        .all(|p| !p.is_empty() && panels.insert(p.clone()))
            }
            DockNode::Split {
                ratio,
                first,
                second,
                ..
            } => {
                ratio.is_finite()
                    && (0.1..=0.9).contains(ratio)
                    && visit(first, areas, panels, depth + 1)
                    && visit(second, areas, panels, depth + 1)
            }
        }
    }
    if !visit(root, &mut BTreeSet::new(), &mut BTreeSet::new(), 0) {
        return Err("ARA7042: invalid dock IDs, active tab, split ratio or depth".into());
    }
    Ok(())
}
