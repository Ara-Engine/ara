//! Invert pass accesses once instead of searching every pass for every resource.
use super::*;

pub(super) struct Index {
    offsets: Vec<usize>,
    entries: Vec<(usize, Access)>,
}
impl Index {
    pub fn new(graph: &Graph) -> Result<Self> {
        let resources = graph.resources.len();
        let mut offsets = vec![0; resources + 1];
        let mut seen = vec![usize::MAX; resources];
        for (p, pass) in graph.passes.iter().enumerate() {
            for &(id, _) in &pass.uses {
                ensure!(id.0 < resources, "Unknown graph resource");
                ensure!(
                    seen[id.0] != p,
                    "Declare one combined access per pass/resource"
                );
                seen[id.0] = p;
                offsets[id.0 + 1] += 1;
            }
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut cursor = offsets[..resources].to_vec();
        let mut entries = vec![(0, Access::Undefined); offsets[resources]];
        // Declaration order defines successive writes. Keep that order within
        // each resource, regardless of its position in a pass's binding list.
        for (p, pass) in graph.passes.iter().enumerate() {
            for &(id, access) in &pass.uses {
                entries[cursor[id.0]] = (p, access);
                cursor[id.0] += 1;
            }
        }
        Ok(Self { offsets, entries })
    }

    pub fn get(&self, resource: usize) -> &[(usize, Access)] {
        &self.entries[self.offsets[resource]..self.offsets[resource + 1]]
    }
}
