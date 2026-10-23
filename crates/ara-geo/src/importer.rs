use crate::*;
use ara_asset::{Artifact, ArtifactKind, ImportContext, ImportInput, ImportOutput, Importer};

/// Worker-installed plugin for an extracted single-material JSON SourceMesh.
/// glTF/FBX workers can call cook directly on their decoded mesh extraction.
pub struct GeoImporter;
impl Importer for GeoImporter {
    fn id(&self) -> &str {
        "ara.geo"
    }
    fn version(&self) -> &str {
        "1-meshopt0.7-dag1"
    }
    fn import(
        &self,
        input: &ImportInput,
        context: &mut ImportContext<'_>,
    ) -> ara_asset::Result<ImportOutput> {
        context.report(0.05, "decoding static geometry")?;
        let source: SourceMesh = serde_json::from_slice(&input.source)?;
        let settings: Settings = serde_json::from_value(input.settings.clone())?;
        context.report(0.1, "building border-locked cluster DAG")?;
        let geometry = cook(&source, settings)
            .and_then(|g| g.encode())
            .map_err(|e| ara_asset::Error::Invalid(e.to_string()))?;
        context.report(0.95, "validating virtual geometry")?;
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "geometry.arag".into(),
                kind: ArtifactKind::Custom,
                bytes: geometry,
            }],
        })
    }
}
