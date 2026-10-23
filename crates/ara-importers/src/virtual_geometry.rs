use crate::{MeshSettings, Vertex, invalid};
use ara_asset::{Artifact, ArtifactKind, Result};
use ara_geo::{SourceMesh, Vertex as GeoVertex};
use serde_json::{Value, json};

/// One material primitive is an independent DAG, so simplification cannot cross
/// material boundaries. The manifest associates it with the classic subasset.
pub(crate) fn cook(
    name: &str,
    vertices: &[Vertex],
    indices: &[u32],
    settings: &MeshSettings,
    deformed: bool,
    materials: usize,
) -> Result<(Option<Artifact>, Value)> {
    if deformed || !settings.virtual_geometry {
        return Ok((
            None,
            json!({"path":"classic","note":if deformed {
            "Skinned or morph-deformed geometry uses the classic path."
        } else { "Virtual geometry disabled by import settings." }}),
        ));
    }
    // Compact each material subset before quantization; unrelated vertices must
    // not enlarge its bounds or consume precision. Preserve UV/normal seams.
    let mut indices = indices.to_vec();
    let compact = meshopt::optimize_vertex_fetch(&mut indices, vertices);
    let source = SourceMesh {
        vertices: compact
            .iter()
            .map(|v| GeoVertex {
                position: v.position,
                normal: crate::mesh::normalize(v.normal),
                uv: v.uv0,
            })
            .collect(),
        indices,
    };
    let geometry = ara_geo::cook(&source, ara_geo::Settings::default()).map_err(invalid)?;
    let mut diagnostics = ara_geo::source_warnings(&source, materials);
    diagnostics.extend(geometry.warnings());
    let artifact_name = name.trim_end_matches(".mesh").to_owned() + ".arag";
    let metadata = json!({"path":"virtual","artifact":artifact_name,
        "cacheVersion":ara_geo::VERSION,"diagnostics":diagnostics});
    Ok((
        Some(Artifact {
            name: artifact_name,
            kind: ArtifactKind::Custom,
            bytes: geometry.encode().map_err(invalid)?,
        }),
        metadata,
    ))
}
