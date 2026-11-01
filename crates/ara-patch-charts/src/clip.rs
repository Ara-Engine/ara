//! Fixed-order double-precision clipping; only final records are rounded to FP32.

use crate::Error;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Vertex {
    pub position: [f64; 3],
    pub bary: [f64; 2],
}

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn position_key(p: [f64; 3]) -> [u64; 3] {
    p.map(|v| if v == 0.0 { 0 } else { v.to_bits() })
}

fn intersection(mut a: Vertex, mut b: Vertex, axis: usize, plane: f64) -> Vertex {
    // Canonical edge direction makes shared-edge intersections bit-identical,
    // including triangles separated by UV/normal seams in the source vertex array.
    if position_key(a.position) > position_key(b.position) {
        std::mem::swap(&mut a, &mut b);
    }
    let t = (plane - a.position[axis]) / (b.position[axis] - a.position[axis]);
    let mut result = Vertex {
        position: std::array::from_fn(|i| a.position[i] + t * (b.position[i] - a.position[i])),
        bary: std::array::from_fn(|i| a.bary[i] + t * (b.bary[i] - a.bary[i])),
    };
    result.position[axis] = plane;
    result
}

fn halfspace(input: &[Vertex], axis: usize, plane: f64, lower: bool) -> Vec<Vertex> {
    let mut output = Vec::with_capacity(input.len() + 1);
    let Some(&last) = input.last() else {
        return output;
    };
    let inside = |v: Vertex| {
        if lower {
            v.position[axis] >= plane
        } else {
            v.position[axis] <= plane
        }
    };
    let mut previous = last;
    for &current in input {
        if inside(previous) != inside(current) {
            output.push(intersection(previous, current, axis, plane));
        }
        if inside(current) {
            output.push(current);
        }
        previous = current;
    }
    output.dedup_by(|a, b| position_key(a.position) == position_key(b.position));
    if output.len() > 1
        && position_key(output[0].position) == position_key(output[output.len() - 1].position)
    {
        output.pop();
    }
    output
}

pub(crate) struct Polygon {
    pub cell: [i32; 3],
    pub vertices: Vec<Vertex>,
}

/// Split by one axis at a time, avoiding a cubic enumeration of a triangle's AABB.
pub(crate) fn cells(
    vertices: [Vertex; 3],
    edge: f64,
    budget: usize,
) -> Result<Vec<Polygon>, Error> {
    let mut polygons = vec![Polygon {
        cell: [0; 3],
        vertices: vertices.to_vec(),
    }];
    for axis in 0..3 {
        let mut next = Vec::new();
        for polygon in polygons {
            let min = polygon
                .vertices
                .iter()
                .map(|v| v.position[axis])
                .fold(f64::INFINITY, f64::min);
            let max = polygon
                .vertices
                .iter()
                .map(|v| v.position[axis])
                .fold(f64::NEG_INFINITY, f64::max);
            let first = (min / edge).floor();
            // A surface exactly on a grid plane belongs to the positive-side cell.
            let last = ((max / edge).ceil() - 1.0).max(first);
            if first < i32::MIN as f64 || last >= i32::MAX as f64 {
                return Err(Error::CoordinateRange);
            }
            if last - first + 1.0 > budget as f64 {
                return Err(Error::FragmentLimit);
            }
            for cell in first as i32..=last as i32 {
                let clipped = halfspace(&polygon.vertices, axis, f64::from(cell) * edge, true);
                let clipped = halfspace(&clipped, axis, (f64::from(cell) + 1.0) * edge, false);
                if clipped.len() >= 3 {
                    let mut key = polygon.cell;
                    key[axis] = cell;
                    next.push(Polygon {
                        cell: key,
                        vertices: clipped,
                    });
                    if next.len() > budget {
                        return Err(Error::FragmentLimit);
                    }
                }
            }
        }
        polygons = next;
    }
    Ok(polygons)
}
