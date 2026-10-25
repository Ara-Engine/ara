//! Geometry is independent of texture quality. Reuse both classic LODs and ARAG
//! data when a project/texture preset changes, without keeping native scenes alive.
use ara_asset::{Artifact, ArtifactKind, CacheBackend, Digest, ImportInput, ImportOutput, Result};
use serde_json::Value;

const META: &str = "__geometry_metadata.json";
pub(crate) struct Geometry {
    pub(crate) artifacts: Vec<Artifact>,
    pub(crate) meshes: Vec<Value>,
}
pub(crate) fn key(input: &ImportInput, settings: &crate::MeshSettings) -> Result<Digest> {
    let mut settings = serde_json::to_value(settings)?;
    if let Some(settings) = settings.as_object_mut() {
        settings.remove("texturePresets");
    }
    let dependencies: Vec<_> = input
        .dependencies
        .iter()
        .map(|(path, bytes)| (path, Digest::of(bytes)))
        .collect();
    let value = serde_json::json!([
        "Ara geometry stage v1",
        input.importer,
        input.version,
        Digest::of(&input.source),
        dependencies,
        settings,
        input.target
    ]);
    Ok(Digest::of(crate::json_bytes(&value)?.as_slice()))
}
pub(crate) fn load(
    memo: Option<&dyn CacheBackend>,
    key: Option<Digest>,
) -> Result<Option<Geometry>> {
    let (Some(memo), Some(key)) = (memo, key) else {
        return Ok(None);
    };
    let Some(mut hit) = memo.load(key)? else {
        return Ok(None);
    };
    let index = hit
        .output
        .artifacts
        .iter()
        .position(|artifact| artifact.name == META)
        .ok_or_else(|| crate::invalid("geometry memo lacks metadata"))?;
    let meta = hit.output.artifacts.remove(index);
    let meshes = serde_json::from_slice(&meta.bytes)?;
    Ok(Some(Geometry {
        artifacts: hit.output.artifacts,
        meshes,
    }))
}
pub(crate) fn store(
    memo: Option<&dyn CacheBackend>,
    key: Option<Digest>,
    artifacts: &[Artifact],
    meshes: &[Value],
) -> Result<()> {
    if let (Some(memo), Some(key)) = (memo, key) {
        let mut artifacts = artifacts.to_vec();
        artifacts.push(Artifact {
            name: META.into(),
            kind: ArtifactKind::Scene,
            bytes: crate::json_bytes(&meshes)?,
        });
        memo.store(key, &ImportOutput { artifacts })?;
    }
    Ok(())
}
