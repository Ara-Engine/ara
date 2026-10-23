//! Production source importers installed only in the disposable import worker.
#![deny(missing_docs)]

mod fbx;
mod gltf;
mod mesh;
mod texture;

pub use fbx::FbxImporter;
pub use gltf::GltfImporter;
pub use mesh::{MeshSettings, Vertex, cook_mesh};
pub use texture::{TextureImporter, TextureSettings, TextureUsage, cook_texture};

use ara_asset::{ImporterRegistry, Result};
/// Builtin registry; other tracks add their importer before calling `serve_worker`.
pub fn registry() -> Result<ImporterRegistry> {
    let mut registry = ImporterRegistry::default();
    registry.register(GltfImporter)?;
    registry.register(FbxImporter)?;
    registry.register(TextureImporter)?;
    Ok(registry)
}

fn invalid(e: impl std::fmt::Display) -> ara_asset::Error {
    ara_asset::Error::Invalid(e.to_string())
}
fn json_bytes(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    Ok(
        ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(value)?))
            .into_bytes(),
    )
}
