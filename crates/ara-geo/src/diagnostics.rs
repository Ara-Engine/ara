use crate::{Geometry, ROOT, SourceMesh};

/// Diagnoses source geometry before quantization. Thresholds are relative to
/// asset size; warnings never silently delete authored triangles.
pub fn source_warnings(source: &SourceMesh, materials: usize) -> Vec<Diagnostic> {
    let mut result = Vec::new();
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for vertex in &source.vertices {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(f64::from(vertex.position[axis]));
            maximum[axis] = maximum[axis].max(f64::from(vertex.position[axis]));
        }
    }
    let diagonal2 = (0..3)
        .map(|i| (maximum[i] - minimum[i]).powi(2))
        .sum::<f64>();
    let mut degenerate = 0u64;
    let mut thin = 0u64;
    let mut huge = 0u64;
    for tri in source.indices.as_chunks::<3>().0 {
        let Some(a) = source.vertices.get(tri[0] as usize) else {
            continue;
        };
        let Some(b) = source.vertices.get(tri[1] as usize) else {
            continue;
        };
        let Some(c) = source.vertices.get(tri[2] as usize) else {
            continue;
        };
        let u: [f64; 3] =
            std::array::from_fn(|i| f64::from(b.position[i]) - f64::from(a.position[i]));
        let v: [f64; 3] =
            std::array::from_fn(|i| f64::from(c.position[i]) - f64::from(a.position[i]));
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let area2 = cross.iter().map(|x| x * x).sum::<f64>().sqrt();
        let longest2 = u
            .iter()
            .map(|x| x * x)
            .sum::<f64>()
            .max(v.iter().map(|x| x * x).sum::<f64>())
            .max((0..3).map(|i| (u[i] - v[i]).powi(2)).sum::<f64>());
        if area2 <= diagonal2 * 1e-14 {
            degenerate += 1;
        } else if area2 < longest2 * 0.001 {
            thin += 1;
        }
        if longest2 > diagonal2 * 0.25 {
            huge += 1;
        }
    }
    for (count, code, advice) in [
        (
            degenerate,
            "ARA_GEO_DEGENERATE",
            "zero-area triangles; repair duplicate or collinear vertices",
        ),
        (
            thin,
            "ARA_GEO_THIN",
            "sliver triangles with aspect ratio above 1000; retopologize thin regions",
        ),
        (
            huge,
            "ARA_GEO_HUGE_TRIANGLES",
            "triangles span more than half the asset bounds; subdivide broad surfaces for efficient culling",
        ),
    ] {
        if count > 0 {
            result.push(Diagnostic {
                code,
                message: format!("{count} {advice}"),
            });
        }
    }
    if materials > 32 {
        result.push(Diagnostic { code:"ARA_GEO_MANY_MATERIALS", message:format!("{materials} material primitives prevent shared cluster groups; consolidate materials where appropriate") });
    }
    result
}
/// Actionable cook-quality warning suitable for an asset inspector or agent.
#[derive(serde::Serialize)]
pub struct Diagnostic {
    /// Stable identifier, independent of formatted measurements.
    pub code: &'static str,
    /// Explanation and suggested authoring check.
    pub message: String,
}
impl Geometry {
    /// Reports low meshlet occupancy and ineffective simplification without
    /// rejecting legitimate disconnected or heavily seamed source geometry.
    pub fn warnings(&self) -> Vec<Diagnostic> {
        let mut result = Vec::new();
        let average = self.diagnostics.source_triangles as f64
            / f64::from(self.diagnostics.leaf_clusters.max(1));
        if self.diagnostics.source_triangles > 2048 && average < 32.0 {
            result.push(Diagnostic{code:"ARA_GEO_LOW_OCCUPANCY",message:format!("{average:.1} triangles per leaf cluster; inspect disconnected triangles and vertex/material seams")});
        }
        let root_triangles: u64 = self
            .clusters
            .iter()
            .filter(|c| c.parent == ROOT)
            .map(|c| u64::from(c.triangle_count))
            .sum();
        if self.diagnostics.source_triangles > 2048
            && root_triangles * 2 > self.diagnostics.source_triangles
        {
            result.push(Diagnostic{code:"ARA_GEO_POOR_REDUCTION",message:format!("{root_triangles} of {} source triangles remain in pinned roots; inspect locked borders, split vertices and nonmanifold topology",self.diagnostics.source_triangles)});
        }
        result
    }
}
