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
/// A dock tree in its own OS window, owned by the main window (D23).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Floating {
    /// Stable window identity, persisted with the layout.
    pub id: String,
    /// Outer position and size in desktop logical pixels: x, y, width, height.
    pub rect: [i32; 4],
    /// The window's dock tree.
    pub root: DockNode,
}
/// Persisted form: the main tree, floating windows and collapsed areas. A bare
/// `DockNode` is still accepted (and written when nothing floats or collapses) for
/// older layout files.
#[derive(Serialize, Deserialize)]
struct Saved {
    main: DockNode,
    floating: Vec<Floating>,
    /// Areas shown as just their tab strip.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    collapsed: BTreeSet<String>,
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
/// Complete persisted layout with a validated reset baseline. Panel and area
/// IDs are unique across the main tree and every floating window.
pub struct DockLayout {
    root: DockNode,
    default: DockNode,
    floating: Vec<Floating>,
    /// Areas shown as just their tab strip (see [`set_collapsed`](Self::set_collapsed)).
    collapsed: BTreeSet<String>,
    default_collapsed: BTreeSet<String>,
}
impl DockLayout {
    /// Validate initial IDs/tabs/ratios before accepting a layout.
    pub fn new(root: DockNode) -> Result<Self> {
        Self::with_floating(root, Vec::new())
    }
    /// A layout whose panels may also live in floating windows.
    pub fn with_floating(root: DockNode, floating: Vec<Floating>) -> Result<Self> {
        validate_all(&root, &floating)?;
        Ok(Self {
            default: root.clone(),
            root,
            floating,
            collapsed: BTreeSet::new(),
            default_collapsed: BTreeSet::new(),
        })
    }
    /// The same layout with `areas` collapsed now and after [`reset`](Self::reset)
    /// (a registered default's collapsed areas).
    pub fn collapsed_by_default(mut self, areas: &[&str]) -> Self {
        self.default_collapsed = areas.iter().map(|a| (*a).to_owned()).collect();
        self.collapsed = self.default_collapsed.clone();
        self
    }
    /// The same layout with exactly `areas` collapsed (restored editor state).
    pub fn with_collapsed(mut self, areas: BTreeSet<String>) -> Self {
        self.collapsed = areas;
        self
    }
    /// Collapsed areas that exist in some window.
    pub fn collapsed(&self) -> BTreeSet<String> {
        let ids = area_ids(&self.root, &self.floating);
        self.collapsed.intersection(&ids).cloned().collect()
    }
    /// Show an area as just its tab strip (`true`), or in full again.
    pub fn set_collapsed(&mut self, area: &str, collapsed: bool) -> Result<()> {
        if !area_ids(&self.root, &self.floating).contains(area) {
            return Err("ARA4042: dock area not found".into());
        }
        if collapsed {
            self.collapsed.insert(area.to_owned());
        } else {
            self.collapsed.remove(area);
        }
        Ok(())
    }
    /// Expand the area holding `panel`, if it is collapsed.
    pub fn expand_panel(&mut self, panel: &str) {
        fn find<'a>(n: &'a DockNode, panel: &str) -> Option<&'a str> {
            match n {
                DockNode::Area { id, tabs, .. } => tabs.iter().any(|t| t == panel).then_some(id),
                DockNode::Split { first, second, .. } => {
                    find(first, panel).or_else(|| find(second, panel))
                }
            }
        }
        let area = find(&self.root, panel)
            .or_else(|| self.floating.iter().find_map(|f| find(&f.root, panel)))
            .map(str::to_owned);
        if let Some(area) = area {
            self.collapsed.remove(&area);
        }
    }
    /// Current serializable layout.
    pub fn root(&self) -> &DockNode {
        &self.root
    }
    /// Floating windows, in creation order.
    pub fn floating(&self) -> &[Floating] {
        &self.floating
    }
    /// Open an absent panel directly in its own window, preserving existing tabs
    /// and their active selections. Invalid or duplicate identities leave the layout unchanged.
    pub fn open_floating(&mut self, panel: &str, id: &str, rect: [i32; 4]) -> Result<()> {
        let mut floating = self.floating.clone();
        floating.push(Floating {
            id: id.into(),
            rect,
            root: DockNode::Area {
                id: format!("{id}-area"),
                tabs: vec![panel.into()],
                active: 0,
            },
        });
        validate_all(&self.root, &floating)?;
        self.floating = floating;
        Ok(())
    }
    /// Tear `panel` off into a new floating window `id` at `rect`.
    pub fn float(&mut self, panel: &str, id: &str, rect: [i32; 4]) -> Result<()> {
        let (mut root, mut floating) = (self.root.clone(), self.floating.clone());
        if !remove_everywhere(&mut root, &mut floating, panel) {
            return Err("ARA4042: dragged panel not found".into());
        }
        floating.push(Floating {
            id: id.into(),
            rect,
            root: DockNode::Area {
                id: format!("{id}-area"),
                tabs: vec![panel.into()],
                active: 0,
            },
        });
        prune_all(&mut root, &mut floating);
        validate_all(&root, &floating)?;
        (self.root, self.floating) = (root, floating);
        Ok(())
    }
    /// Remove a panel from every window; empty areas collapse and empty windows close.
    pub fn close(&mut self, panel: &str) -> Result<()> {
        let (mut root, mut floating) = (self.root.clone(), self.floating.clone());
        if !remove_everywhere(&mut root, &mut floating, panel) {
            return Err("ARA4042: panel is not open".into());
        }
        prune_all(&mut root, &mut floating);
        (self.root, self.floating) = (root, floating);
        Ok(())
    }
    /// Record a floating window's new position or size.
    pub fn set_rect(&mut self, id: &str, rect: [i32; 4]) -> Result<()> {
        if rect[2] <= 0 || rect[3] <= 0 {
            return Err("ARA4042: floating windows need a positive size".into());
        }
        let window = self
            .floating
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or("ARA4042: floating window not found")?;
        window.rect = rect;
        Ok(())
    }
    /// Whether a panel is open in any window.
    pub fn contains(&self, panel: &str) -> bool {
        fn visit(n: &DockNode, p: &str) -> bool {
            match n {
                DockNode::Area { tabs, .. } => tabs.iter().any(|t| t == p),
                DockNode::Split { first, second, .. } => visit(first, p) || visit(second, p),
            }
        }
        visit(&self.root, panel) || self.floating.iter().any(|f| visit(&f.root, panel))
    }
    /// Update a persisted split after dragging its runtime splitter; path entries choose first/second.
    pub fn resize_split(&mut self, path: &[bool], ratio: f32) -> Result<()> {
        self.resize_split_in(None, path, ratio)
    }
    /// Resize a split in the main tree (`None`) or a floating window's own tree.
    /// Unknown windows, invalid paths and non-finite ratios leave every tree unchanged.
    /// Finite fractions retain the shared 0.1–0.9 limit.
    pub fn resize_split_in(
        &mut self,
        window: Option<&str>,
        path: &[bool],
        ratio: f32,
    ) -> Result<()> {
        if !ratio.is_finite() {
            return Err("ARA4042: invalid split ratio".into());
        }
        let mut node = match window {
            None => &mut self.root,
            Some(id) => {
                &mut self
                    .floating
                    .iter_mut()
                    .find(|window| window.id == id)
                    .ok_or("ARA4042: floating window not found")?
                    .root
            }
        };
        for second in path {
            let DockNode::Split {
                first,
                second: other,
                ..
            } = node
            else {
                return Err("ARA4042: split path crosses a tab area".into());
            };
            node = if *second { other } else { first };
        }
        let DockNode::Split { ratio: value, .. } = node else {
            return Err("ARA4042: resize target is not a split".into());
        };
        *value = ratio.clamp(0.1, 0.9);
        Ok(())
    }
    /// Restore the registered default layout; floating windows close.
    pub fn reset(&mut self) {
        self.root = self.default.clone();
        self.floating.clear();
        self.collapsed = self.default_collapsed.clone();
    }
    /// Stable JSON serialization; field order follows the typed enum schema.
    pub fn serialize(&self) -> Result<String> {
        let collapsed = self.collapsed();
        let value = if self.floating.is_empty() && collapsed.is_empty() {
            ara_json::to_value(&&self.root)?
        } else {
            ara_json::to_value(&Saved {
                main: self.root.clone(),
                floating: self.floating.clone(),
                collapsed,
            })?
        };
        Ok(ara_json::to_canonical_string(&value))
    }
    /// Atomically load a saved layout, retaining the original reset baseline. A
    /// file from before collapsible areas collapses none.
    pub fn restore(&mut self, json: &str) -> Result<()> {
        let value: serde_json::Value = serde_json::from_value(ara_json::parse(json)?)?;
        let (root, floating, collapsed) = if value.get("main").is_some() {
            let saved: Saved = serde_json::from_value(value)?;
            (saved.main, saved.floating, saved.collapsed)
        } else {
            (serde_json::from_value(value)?, Vec::new(), BTreeSet::new())
        };
        validate_all(&root, &floating)?;
        self.root = root;
        self.floating = floating;
        self.collapsed = collapsed;
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
        if !visit(&mut self.root, panel)
            && !self.floating.iter_mut().any(|f| visit(&mut f.root, panel))
        {
            return Err("ARA4042: unknown dock panel".into());
        }
        Ok(())
    }
    /// Drop a tab into another area or onto a split edge, in any window; failures
    /// leave the layout intact. Emptied areas collapse and emptied windows close.
    pub fn drop_tab(&mut self, panel: &str, target: DockDrop) -> Result<()> {
        let (mut candidate, mut floating) = (self.root.clone(), self.floating.clone());
        if !remove_everywhere(&mut candidate, &mut floating, panel) {
            return Err("ARA4042: dragged panel not found".into());
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
        let destination = match find(&mut candidate, area) {
            Some(d) => d,
            None => floating
                .iter_mut()
                .find_map(|f| find(&mut f.root, area))
                .ok_or("ARA4042: dock destination missing")?,
        };
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
        prune_all(&mut candidate, &mut floating);
        validate_all(&candidate, &floating)?;
        self.root = candidate;
        self.floating = floating;
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
/// Every area ID in the main tree and the floating windows.
fn area_ids(root: &DockNode, floating: &[Floating]) -> BTreeSet<String> {
    fn visit(n: &DockNode, out: &mut BTreeSet<String>) {
        match n {
            DockNode::Area { id, .. } => {
                out.insert(id.clone());
            }
            DockNode::Split { first, second, .. } => {
                visit(first, out);
                visit(second, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    visit(root, &mut out);
    for f in floating {
        visit(&f.root, &mut out);
    }
    out
}
/// Remove a panel from whichever tree holds it.
fn remove_everywhere(root: &mut DockNode, floating: &mut [Floating], panel: &str) -> bool {
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
    remove(root, panel) || floating.iter_mut().any(|f| remove(&mut f.root, panel))
}
/// Collapse splits around empty areas; the main root itself always survives.
fn prune_all(root: &mut DockNode, floating: &mut Vec<Floating>) {
    fn prune(n: DockNode) -> Option<DockNode> {
        match n {
            DockNode::Area { ref tabs, .. } if tabs.is_empty() => None,
            DockNode::Area { .. } => Some(n),
            DockNode::Split {
                axis,
                ratio,
                first,
                second,
            } => match (prune(*first), prune(*second)) {
                (Some(a), Some(b)) => Some(DockNode::Split {
                    axis,
                    ratio,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }
    if let Some(pruned) = prune(root.clone()) {
        *root = pruned;
    }
    floating.retain_mut(|f| match prune(f.root.clone()) {
        Some(pruned) => {
            f.root = pruned;
            true
        }
        None => false,
    });
}
fn validate_all(root: &DockNode, floating: &[Floating]) -> Result<()> {
    let mut areas = BTreeSet::new();
    let mut panels = BTreeSet::new();
    for node in std::iter::once(root).chain(floating.iter().map(|f| &f.root)) {
        validate_into(node, &mut areas, &mut panels)?;
    }
    let mut windows = BTreeSet::new();
    if !floating
        .iter()
        .all(|f| !f.id.is_empty() && windows.insert(&f.id) && f.rect[2] > 0 && f.rect[3] > 0)
    {
        return Err("ARA4042: invalid or duplicate floating window".into());
    }
    Ok(())
}
fn validate_into(
    root: &DockNode,
    areas: &mut BTreeSet<String>,
    panels: &mut BTreeSet<String>,
) -> Result<()> {
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
    if !visit(root, areas, panels, 0) {
        return Err("ARA4042: invalid dock IDs, active tab, split ratio or depth".into());
    }
    Ok(())
}
#[cfg(test)]
#[path = "docking/resize_tests.rs"]
mod resize_tests;
#[cfg(test)]
mod tests {
    use super::*;
    fn area(id: &str, tabs: &[&str]) -> DockNode {
        DockNode::Area {
            id: id.into(),
            tabs: tabs.iter().map(|t| t.to_string()).collect(),
            active: 0,
        }
    }
    #[test]
    fn opening_a_floating_panel_preserves_existing_views_and_rejects_duplicates_atomically()
    -> Result<()> {
        for active in [0, 1] {
            let mut root = area("view", &["scene", "game"]);
            if let DockNode::Area {
                active: selected, ..
            } = &mut root
            {
                *selected = active;
            }
            let mut dock = DockLayout::new(root.clone())?;
            dock.open_floating("settings", "w1", [10, 20, 960, 640])?;
            assert_eq!(dock.root(), &root);
            let before = dock.serialize()?;
            for (panel, window, size) in [
                ("settings", "w2", [0, 0, 100, 100]),
                ("profiler", "w1", [0, 0, 100, 100]),
                ("profiler", "w2", [0, 0, 0, 100]),
            ] {
                assert!(dock.open_floating(panel, window, size).is_err());
                assert_eq!(dock.serialize()?, before);
            }
            let mut restored = DockLayout::new(area("empty", &[]))?;
            restored.restore(&before)?;
            assert_eq!(restored.root(), &root);
            assert_eq!(restored.floating(), dock.floating());
        }
        Ok(())
    }
    #[test]
    fn panels_float_redock_and_round_trip() -> Result<()> {
        let root = DockNode::Split {
            axis: Axis::Horizontal,
            ratio: 0.3,
            first: Box::new(area("left", &["hierarchy"])),
            second: Box::new(area("view", &["scene", "inspector"])),
        };
        let mut dock = DockLayout::new(root)?;
        dock.float("hierarchy", "w1", [100, 100, 320, 480])?;
        // The emptied left area collapsed; the panel now lives in the window.
        assert!(matches!(dock.root(), DockNode::Area { id, .. } if id == "view"));
        assert_eq!(dock.floating().len(), 1);
        let saved = dock.serialize()?;
        let mut copy = DockLayout::new(area("x", &[]))?;
        copy.restore(&saved)?;
        assert_eq!(copy.floating(), dock.floating());
        // Docking back closes the empty floating window.
        dock.drop_tab(
            "hierarchy",
            DockDrop::Tab {
                area: "view".into(),
                index: 0,
            },
        )?;
        assert!(dock.floating().is_empty());
        assert!(dock.contains("hierarchy"));
        // A panel cannot be in two windows at once.
        dock.float("inspector", "w2", [0, 0, 300, 300])?;
        assert!(
            DockLayout::with_floating(area("v", &["inspector"]), dock.floating().to_vec()).is_err()
        );
        Ok(())
    }
}
