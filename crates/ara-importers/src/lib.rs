//! Production source importers installed only in the disposable import worker.
#![deny(missing_docs)]

pub mod audio;
pub mod commands;
mod fbx;
mod gltf;
mod mesh;
mod texture;
mod texture_codec;
mod virtual_geometry;

pub use fbx::FbxImporter;
pub use gltf::GltfImporter;
pub use mesh::{MeshSettings, Vertex, cook_mesh};
pub use texture::{
    TextureImporter, TextureSettings, TextureUsage, cook_texture, cook_texture_with_preset,
};

use ara_asset::{ImporterRegistry, Result};
/// Builtin registry; other tracks add their importer before calling `serve_worker`.
pub fn registry() -> Result<ImporterRegistry> {
    let mut registry = ImporterRegistry::default();
    registry.register(GltfImporter)?;
    registry.register(FbxImporter)?;
    registry.register(TextureImporter)?;
    registry.register(audio::AudioAsset::default())?;
    registry.register(ara_geo::GeoImporter)?;
    Ok(registry)
}

fn invalid(e: impl std::fmt::Display) -> ara_asset::Error {
    ara_asset::Error::Invalid(e.to_string())
}
fn json_bytes(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    Ok(
        ara_json::to_canonical_string(&ara_json::sort_keys(&ara_json::to_value(value)?))
            .into_bytes(),
    )
}
