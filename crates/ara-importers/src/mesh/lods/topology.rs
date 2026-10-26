//! Geometric borders and per-sheet area budgets survive attribute vertex splits.
use super::Vertex;
use meshopt::SimplifyVertex;
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, DefaultHasher},
};
mod cards;

// IDs are assigned in source order, never bucket order. The only map iteration
// below ORs border bits; all floating-point reductions retain triangle order.
// A fixed hasher avoids consulting randomness in the pure importer.
type Lookup<K, V> = HashMap<K, V, BuildHasherDefault<DefaultHasher>>;

pub(super) struct Topology {
    pub(super) locks: Vec<SimplifyVertex>,
    points: Vec<usize>,
    membership: Vec<Vec<usize>>,
    areas: Vec<f64>,
    watched: Vec<bool>,
}

fn area(vertices: &[Vertex], triangle: &[u32; 3]) -> f64 {
    let p = triangle.map(|i| vertices[i as usize].position.map(f64::from));
    let a: [f64; 3] = std::array::from_fn(|i| p[1][i] - p[0][i]);
    let b: [f64; 3] = std::array::from_fn(|i| p[2][i] - p[0][i]);
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    cross.iter().map(|x| x * x).sum::<f64>().sqrt() * 0.5
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

impl Topology {
    pub(super) fn new(vertices: &[Vertex], indices: &[u32]) -> Self {
        let mut positions = Lookup::default();
        let mut first = Vec::<usize>::new();
        let mut seam = Vec::new();
        let mut points = Vec::with_capacity(vertices.len());
        for (index, vertex) in vertices.iter().enumerate() {
            let key = vertex
                .position
                .map(|v| if v == 0.0 { 0 } else { v.to_bits() });
            let next = positions.len();
            let point = *positions.entry(key).or_insert_with(|| {
                first.push(index);
                seam.push(false);
                next
            });
            let other = &vertices[first[point]];
            seam[point] |= vertex.uv0 != other.uv0
                || vertex.uv1 != other.uv1
                || vertex.tangent[3] != other.tangent[3]
                || vertex.color != other.color;
            points.push(point);
        }
        let triangles = indices.as_chunks::<3>().0;
        let mut parent: Vec<_> = (0..triangles.len()).collect();
        let mut edges = Lookup::<(usize, usize), (usize, usize)>::default();
        for (number, triangle) in triangles.iter().enumerate() {
            let ids = triangle.map(|i| points[i as usize]);
            for (a, b) in [(ids[0], ids[1]), (ids[1], ids[2]), (ids[2], ids[0])] {
                if a == b {
                    continue;
                }
                let (owner, count) = edges.entry((a.min(b), a.max(b))).or_insert((number, 0));
                let (a, b) = (find(&mut parent, *owner), find(&mut parent, number));
                parent[a.max(b)] = a.min(b);
                *count += 1;
            }
        }
        let mut components = Lookup::default();
        let ids: Vec<_> = (0..triangles.len())
            .map(|i| {
                let next = components.len();
                *components.entry(find(&mut parent, i)).or_insert(next)
            })
            .collect();
        let mut areas = vec![0.0; components.len()];
        let watched = cards::classify(vertices, triangles, &ids, components.len());
        let mut membership = vec![Vec::new(); positions.len()];
        for (number, triangle) in triangles.iter().enumerate() {
            let component = ids[number];
            areas[component] += area(vertices, triangle);
            for &index in triangle {
                let list = &mut membership[points[index as usize]];
                if !list.contains(&component) {
                    list.push(component);
                }
            }
        }
        for list in &mut membership {
            list.sort_unstable();
        }
        let mut border = vec![false; positions.len()];
        for (&(a, b), &(_, count)) in &edges {
            if count == 1 {
                border[a] = true;
                border[b] = true;
            }
        }
        let locks = points
            .iter()
            .map(|&p| {
                let mut flags = SimplifyVertex::None;
                if border[p] {
                    flags |= SimplifyVertex::Lock;
                }
                if seam[p] {
                    flags |= SimplifyVertex::Protect;
                }
                flags
            })
            .collect();
        Self {
            locks,
            points,
            membership,
            areas,
            watched,
        }
    }

    pub(super) fn preserves_area(&self, vertices: &[Vertex], indices: &[u32]) -> bool {
        // Each thin card has its own budget, including rotated cards with
        // coincident backfaces. Three-dimensional masked stems are not cards.
        let mut areas = vec![0.0; self.areas.len()];
        for triangle in indices.as_chunks::<3>().0 {
            let points = triangle.map(|i| self.points[i as usize]);
            let Some(&component) = self.membership[points[0]].iter().find(|c| {
                self.membership[points[1]].binary_search(c).is_ok()
                    && self.membership[points[2]].binary_search(c).is_ok()
            }) else {
                return false;
            };
            areas[component] += area(vertices, triangle);
        }
        self.areas
            .iter()
            .enumerate()
            .all(|(i, original)| !self.watched[i] || areas[i] + original * 1e-9 >= original * 0.9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sheets() -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        for (offset, size) in [(0.0, 10.0), (20.0, 1.0)] {
            for (x, y) in [(0.0, 0.0), (size, 0.0), (size, size), (0.0, size)] {
                vertices.push(Vertex {
                    position: [x + offset, y, 0.0],
                    ..Vertex::default()
                });
            }
        }
        (vertices, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7])
    }

    #[test]
    fn a_large_sheet_cannot_hide_the_loss_of_a_small_card() {
        let (vertices, indices) = sheets();
        let topology = Topology::new(&vertices, &indices);
        assert!(topology.preserves_area(&vertices, &indices));
        assert!(!topology.preserves_area(&vertices, &indices[..6]));
        assert!(
            topology
                .locks
                .iter()
                .all(|f| f.contains(SimplifyVertex::Lock))
        );
    }

    #[test]
    fn rotated_cards_with_coincident_backfaces_keep_individual_area_budgets() {
        let (mut vertices, mut indices) = sheets();
        for vertex in &mut vertices {
            let [x, y, _] = vertex.position;
            vertex.position = [
                x * std::f32::consts::FRAC_1_SQRT_2 - y / 6.0f32.sqrt(),
                x * std::f32::consts::FRAC_1_SQRT_2 + y / 6.0f32.sqrt(),
                y * (2.0f32 / 3.0).sqrt(),
            ];
        }
        // Interleave front/back faces so the first twelve indices contain only
        // the large sheet. Every edge is closed despite both cards being flat.
        indices = indices
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[a, b, c]| [a, b, c, a, c, b])
            .collect();
        let topology = Topology::new(&vertices, &indices);
        assert!(topology.preserves_area(&vertices, &indices));
        assert!(!topology.preserves_area(&vertices, &indices[..12]));
        assert!(
            topology
                .locks
                .iter()
                .all(|f| !f.contains(SimplifyVertex::Lock))
        );
    }

    #[test]
    fn long_open_tubes_are_not_classified_as_leaf_cards() {
        let mut vertices = Vec::new();
        for height in [0.0, 10.0] {
            for i in 0..16 {
                let angle = i as f32 * std::f32::consts::TAU / 16.0;
                vertices.push(Vertex {
                    position: [angle.cos() * 0.02, height, angle.sin() * 0.02],
                    ..Vertex::default()
                });
            }
        }
        let indices: Vec<_> = (0..16)
            .flat_map(|a| {
                let b = (a + 1) % 16;
                [a, b, a + 16, b, b + 16, a + 16]
            })
            .collect();
        let topology = Topology::new(&vertices, &indices);
        assert_eq!(topology.watched, [false]);
    }
}
