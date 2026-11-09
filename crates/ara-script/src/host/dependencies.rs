//! Incremental read/write hazards preserve system order without a quadratic edge list.
use std::collections::{BTreeMap, BTreeSet};

use super::registry::SystemDecl;

#[derive(Default)]
struct Access {
    writer: Option<usize>,
    readers: Vec<usize>,
}

/// Builds dependencies for valid systems in `(Order, name, id)` order.
pub(super) struct Dependencies {
    accesses: BTreeMap<(bool, u32), Access>,
    systems: Vec<u32>,
    structural: Option<usize>,
    chain: bool,
}

impl Default for Dependencies {
    fn default() -> Self {
        Self {
            accesses: BTreeMap::new(),
            systems: Vec::new(),
            structural: None,
            chain: true,
        }
    }
}

impl Dependencies {
    /// Returns earlier script IDs that cover every read/write/structural hazard.
    pub fn push(&mut self, system: &SystemDecl) -> Vec<u32> {
        let index = self.systems.len();
        let mut accesses = BTreeMap::new();
        // Columns and resources have separate index spaces. Coalesce repeated
        // declarations before updating history so they cannot create self edges.
        for (resource, entries) in [(false, &system.columns), (true, &system.resources)] {
            for &(id, write) in entries {
                *accesses.entry((resource, id)).or_insert(false) |= write;
            }
        }
        let mut dependencies = BTreeSet::new();
        for (key, write) in accesses {
            let access = self.accesses.entry(key).or_default();
            dependencies.extend(access.writer);
            if write {
                dependencies.extend(access.readers.drain(..));
                access.writer = Some(index);
            } else {
                access.readers.push(index);
            }
        }
        if system.structural {
            dependencies.extend(self.structural);
            self.structural = Some(index);
        }
        // Consecutive nodes are ordered iff this entire forward DAG is a chain.
        self.chain &= index == 0 || dependencies.contains(&(index - 1));
        self.systems.push(system.id);
        dependencies
            .into_iter()
            .map(|earlier| self.systems[earlier])
            .collect()
    }

    /// A totally ordered DAG has no outer-system parallelism to exploit.
    pub fn is_chain(&self) -> bool {
        self.chain
    }
}

#[cfg(test)]
#[path = "dependencies_tests.rs"]
mod tests;
