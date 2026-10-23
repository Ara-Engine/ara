use crate::{Geometry, ROOT};
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
