//! Reuse bounded vertex-ID lookups across groups. IDs are assigned by first
//! encounter in the index stream, exactly as with the original ordered map.
use crate::{SourceMesh, Vertex};

#[derive(Clone, Copy, Default)]
struct Slot {
    generation: u32,
    local: u32,
}

pub(super) struct Remap<'a> {
    source: &'a SourceMesh,
    slots: Vec<Slot>,
    generation: u32,
}

impl<'a> Remap<'a> {
    pub fn new(source: &'a SourceMesh) -> Self {
        Self {
            source,
            // Avoid a source-sized allocation for a sparse public SourceMesh.
            // Imported material subsets are compacted before entering this cook.
            slots: if source.vertices.len() <= source.indices.len() {
                vec![Slot::default(); source.vertices.len()]
            } else {
                Vec::new()
            },
            generation: 0,
        }
    }

    pub fn compact(&mut self, indices: &[u32]) -> (SourceMesh, Vec<u32>) {
        let source = self.source;
        if self.slots.is_empty() {
            return sparse(source, indices);
        }
        self.generation = self.generation.checked_add(1).unwrap_or_else(|| {
            for slot in &mut self.slots {
                slot.generation = 0;
            }
            1
        });
        let capacity = source.vertices.len().min(indices.len());
        let mut vertices = Vec::with_capacity(capacity);
        let mut ids = Vec::with_capacity(capacity);
        let mut local = Vec::with_capacity(indices.len());
        for &id in indices {
            let slot = &mut self.slots[id as usize];
            if slot.generation != self.generation {
                slot.generation = self.generation;
                slot.local = vertices.len() as u32;
                vertices.push(source.vertices[id as usize]);
                ids.push(id);
            }
            local.push(slot.local);
        }
        (
            SourceMesh {
                vertices,
                indices: local,
            },
            ids,
        )
    }
}

fn sparse(source: &SourceMesh, indices: &[u32]) -> (SourceMesh, Vec<u32>) {
    let mut map = std::collections::BTreeMap::new();
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut ids = Vec::new();
    let local = indices
        .iter()
        .map(|&id| {
            *map.entry(id).or_insert_with(|| {
                let index = vertices.len() as u32;
                vertices.push(source.vertices[id as usize]);
                ids.push(id);
                index
            })
        })
        .collect();
    (
        SourceMesh {
            vertices,
            indices: local,
        },
        ids,
    )
}

#[cfg(test)]
mod tests;
