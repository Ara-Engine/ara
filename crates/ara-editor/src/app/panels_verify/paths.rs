//! One immutable path snapshot for this fixture's browser and physical endpoints.
use super::*;
pub(super) struct Paths {
    pub mesh: String,
    pub models: String,
    pub checker: String,
    pub textures: String,
    pub material: String,
    pub vfx: String,
    pub samples: String,
    pub long_textures: [String; 2],
}
impl Paths {
    pub(super) fn read(shell: &Shell) -> Option<Self> {
        let at = |logical: &str| {
            shell
                .asset_authored_files()
                .iter()
                .find(|file| {
                    matches!(&file.scope, ara_host::project::AuthoredScope::Authored)
                        && file.path == logical
                })
                .map(|file| file.storage_path.clone())
        };
        Some(Self {
            mesh: at("models/Avocado.glb")?,
            models: at("models")?,
            checker: at("textures/checker.png")?,
            textures: at("textures")?,
            material: at("samples/material.graph")?,
            vfx: at("samples/vfx.graph")?,
            samples: at("samples")?,
            long_textures: [
                at("textures/dry_ground_01_diff_2k.png")?,
                at("textures/rusty_corrugated_iron_nor_gl_2k.png")?,
            ],
        })
    }
}
