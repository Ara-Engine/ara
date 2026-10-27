//! Stable-key selection independent of mounted rows, focus widgets and authoring.
use std::collections::BTreeSet;

/// Native modifier intent; the host maps its primary modifier (Ctrl or Command).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Extend the displayed-order range from the stable anchor.
    pub extend: bool,
    /// Toggle a clicked item, preserve the base of a range, or move keyboard focus
    /// without changing selection when extend is false.
    pub additive: bool,
}

/// Navigation through the complete filtered projection, never a virtual row pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigation {
    /// Previous displayed item.
    Previous,
    /// Next displayed item.
    Next,
    /// First displayed item.
    First,
    /// Last displayed item.
    Last,
    /// Move backward by a viewport's logical row count.
    PreviousPage(usize),
    /// Move forward by a viewport's logical row count.
    NextPage(usize),
}

/// Apply an explicitly provided set, for example a viewport marquee result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOperation {
    /// Replace the selection.
    Replace,
    /// Include every supplied key.
    Add,
    /// Exclude every supplied key.
    Subtract,
    /// Toggle each supplied key exactly once.
    Toggle,
}

struct Range<K> {
    before: BTreeSet<K>,
    active: Option<K>,
    anchor: Option<K>,
    base: BTreeSet<K>,
    additive: bool,
}

/// Selection, range anchor and active item keyed by model identity. A host keeps
/// authoring/navigation state authoritative and commits returned keys through its
/// typed API. Keys may remain selected while filtered out; deleted keys are pruned
/// only when the host supplies an existence predicate to reconcile.
pub struct Selection<K> {
    selected: BTreeSet<K>,
    anchor: Option<K>,
    active: Option<K>,
    range: Option<Range<K>>,
}

impl<K> Default for Selection<K> {
    fn default() -> Self {
        Self {
            selected: BTreeSet::new(),
            anchor: None,
            active: None,
            range: None,
        }
    }
}

impl<K: Clone + Ord> Selection<K> {
    /// Selected model keys in deterministic key order, independent of pool slots.
    pub fn selected(&self) -> &BTreeSet<K> {
        &self.selected
    }

    /// Stable start of a range gesture.
    pub fn anchor(&self) -> Option<&K> {
        self.anchor.as_ref()
    }

    /// Keyboard destination; it can be focused without being selected.
    pub fn active(&self) -> Option<&K> {
        self.active.as_ref()
    }

    /// Set keyboard focus without selecting the item or moving the range anchor.
    /// This starts a fresh gesture, for example after Escape deselects a list.
    pub fn focus(&mut self, key: K) {
        self.active = Some(key);
        self.range = None;
    }

    /// Apply an external selection notification. An echoed set preserves the live
    /// range anchor and focus; a different set starts a new selection gesture.
    pub fn set_selected(&mut self, keys: impl IntoIterator<Item = K>) {
        let mut keys = keys.into_iter();
        let first = keys.next();
        let mut selected: BTreeSet<_> = keys.collect();
        if let Some(key) = &first {
            selected.insert(key.clone());
        }
        if selected == self.selected {
            return;
        }
        self.selected = selected;
        self.range = None;
        if !self
            .active
            .as_ref()
            .is_some_and(|key| self.selected.contains(key))
        {
            self.active = first;
        }
        if !self
            .anchor
            .as_ref()
            .is_some_and(|key| self.selected.contains(key))
        {
            self.anchor = self.active.clone();
        }
    }

    /// Repair after filtering, reordering or deletion. The order is the complete
    /// displayed projection, including unmounted rows; exists addresses the full
    /// model, so filtering does not accidentally delete a hidden selection.
    pub fn reconcile(&mut self, order: &[K], mut exists: impl FnMut(&K) -> bool) {
        self.selected.retain(&mut exists);
        if let Some(range) = &mut self.range {
            range.before.retain(&mut exists);
            range.base.retain(&mut exists);
        }
        if self.anchor.as_ref().is_some_and(|key| !exists(key)) {
            self.anchor = self
                .active
                .as_ref()
                .filter(|key| exists(key) && order.contains(key))
                .cloned()
                .or_else(|| {
                    order
                        .iter()
                        .find(|key| self.selected.contains(*key))
                        .cloned()
                });
            self.range = None;
        }
        if self.active.as_ref().is_some_and(|key| !exists(key)) {
            self.active = order
                .iter()
                .find(|key| self.selected.contains(*key))
                .cloned();
        }
        self.repair_visible(order);
    }

    /// Apply a pointer selection. Shift selects a range; primary-modifier clicks
    /// toggle one item; primary+Shift unions a range with its original base.
    pub fn select(&mut self, order: &[K], key: &K, modifiers: Modifiers) -> bool {
        let Some(target) = order.iter().position(|candidate| candidate == key) else {
            return false;
        };
        self.repair_visible(order);
        if modifiers.extend {
            if self.anchor.is_none() {
                self.anchor = self.active.clone().or_else(|| Some(key.clone()));
            }
            let anchor = self
                .anchor
                .as_ref()
                .and_then(|key| order.iter().position(|candidate| candidate == key))
                .unwrap_or(target);
            if self
                .range
                .as_ref()
                .is_none_or(|range| range.additive != modifiers.additive)
            {
                self.range = Some(Range {
                    before: self.selected.clone(),
                    active: self.active.clone(),
                    anchor: self.anchor.clone(),
                    base: if modifiers.additive {
                        self.selected.clone()
                    } else {
                        BTreeSet::new()
                    },
                    additive: modifiers.additive,
                });
            }
            let range = self
                .range
                .as_ref()
                .expect("range created for an extending gesture");
            self.selected.clone_from(&range.base);
            self.selected.extend(
                order[anchor.min(target)..=anchor.max(target)]
                    .iter()
                    .cloned(),
            );
        } else {
            self.range = None;
            if modifiers.additive {
                if !self.selected.remove(key) {
                    self.selected.insert(key.clone());
                }
            } else {
                self.selected.clear();
                self.selected.insert(key.clone());
            }
            self.anchor = Some(key.clone());
        }
        self.active = Some(key.clone());
        true
    }

    /// Move over all displayed rows and return the active key for virtual-scroll
    /// reveal. Primary-modifier arrows move focus only; Shift extends selection.
    pub fn navigate(
        &mut self,
        order: &[K],
        navigation: Navigation,
        modifiers: Modifiers,
    ) -> Option<K> {
        if order.is_empty() {
            self.repair_visible(order);
            return None;
        }
        self.repair_visible(order);
        let current = self
            .active
            .as_ref()
            .and_then(|key| order.iter().position(|candidate| candidate == key));
        let last = order.len() - 1;
        let next = match navigation {
            Navigation::Previous => current.map_or(last, |i| i.saturating_sub(1)),
            Navigation::Next => current.map_or(0, |i| i.saturating_add(1).min(last)),
            Navigation::First => 0,
            Navigation::Last => last,
            Navigation::PreviousPage(count) => current.unwrap_or(0).saturating_sub(count.max(1)),
            Navigation::NextPage(count) => {
                current.unwrap_or(0).saturating_add(count.max(1)).min(last)
            }
        };
        let key = order[next].clone();
        if modifiers.additive && !modifiers.extend {
            self.range = None;
            self.active = Some(key.clone());
        } else {
            self.select(order, &key, modifiers);
        }
        Some(key)
    }

    /// Apply a marquee or another explicit set once. The final supplied key is
    /// active, independently of whether a subtraction leaves it selected.
    pub fn apply(&mut self, keys: impl IntoIterator<Item = K>, operation: SetOperation) {
        let keys: Vec<_> = keys.into_iter().collect();
        self.range = None;
        if operation == SetOperation::Replace {
            self.selected.clear();
        }
        for key in keys.iter().collect::<BTreeSet<_>>() {
            match operation {
                SetOperation::Replace | SetOperation::Add => {
                    self.selected.insert(key.clone());
                }
                SetOperation::Subtract => {
                    self.selected.remove(key);
                }
                SetOperation::Toggle => {
                    if !self.selected.remove(key) {
                        self.selected.insert(key.clone());
                    }
                }
            }
        }
        self.anchor = keys.first().cloned();
        self.active = keys.last().cloned();
    }

    /// Escape an extending gesture, restoring its pre-range selected set and focus.
    /// A host can call clear when there is no gesture and Escape means deselect.
    pub fn cancel_range(&mut self) -> bool {
        let Some(range) = self.range.take() else {
            return false;
        };
        self.selected = range.before;
        self.active = range.active;
        self.anchor = range.anchor;
        true
    }

    /// Deselect and end the gesture without changing any authored content.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.active = None;
        self.range = None;
    }

    fn repair_visible(&mut self, order: &[K]) {
        let shown = |key: &K| order.contains(key);
        if self.anchor.as_ref().is_some_and(|key| !shown(key)) {
            self.range = None;
            self.anchor = self
                .active
                .as_ref()
                .filter(|key| shown(key))
                .cloned()
                .or_else(|| {
                    order
                        .iter()
                        .find(|key| self.selected.contains(*key))
                        .cloned()
                });
        }
        if self.active.as_ref().is_some_and(|key| !shown(key)) {
            self.active = order
                .iter()
                .find(|key| self.selected.contains(*key))
                .cloned()
                .or_else(|| self.anchor.clone());
        }
    }
}

#[cfg(test)]
mod tests;
