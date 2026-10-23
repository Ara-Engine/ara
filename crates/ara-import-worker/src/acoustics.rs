//! Baked acoustic probes behind the ara-asset importer contract (`ara.acoustics`):
//! the source is an `AcousticCell` blob (`ARAACU01`, written by an ara-acoustics
//! baker); the import validates its checksum and probes and publishes it unchanged as
//! `acoustics.bin`, which play loads and unloads with the cell (`Cell.acoustics`).
use ara_asset::{
    Artifact, ArtifactKind, Error, ImportContext, ImportInput, ImportOutput, Importer,
};

/// The `ara.acoustics` importer.
pub struct AcousticsAsset;

impl Importer for AcousticsAsset {
    fn id(&self) -> &str {
        "ara.acoustics"
    }
    fn version(&self) -> &str {
        // The AcousticCell format version (ARAACU01).
        "1"
    }
    fn import(
        &self,
        input: &ImportInput,
        _: &mut ImportContext<'_>,
    ) -> ara_asset::Result<ImportOutput> {
        ara_acoustics::bake::AcousticCell::decode(&input.source)
            .map_err(|e| Error::Invalid(format!("{}: {e}", input.source_path)))?;
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "acoustics.bin".into(),
                kind: ArtifactKind::Custom,
                bytes: input.source.clone(),
            }],
        })
    }
}
