//! `samples/assets.lock.json`: every third-party file the sample projects use, pinned by
//! URL, size and BLAKE3, with its license and attribution. Binaries never enter git;
//! `fetch` materializes them under `<project>/assets/third_party/<asset>/`.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The whole manifest. Assets are kept sorted by ID so diffs stay small.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lock {
    /// Manifest format version.
    pub version: u32,
    /// Every pinned asset.
    pub assets: Vec<Asset>,
}

/// What an asset is used as; decides how `build` registers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// A glTF/GLB model, imported through a sidecar.
    Model,
    /// A PBR texture set (base colour, normal, ARM), used by generated kit geometry.
    Texture,
    /// An equirectangular HDR environment.
    Hdri,
    /// A skinned, animated glTF character.
    Character,
    /// A sound or music file (WAV/OGG), imported by `ara.audio`.
    Audio,
}

/// One pinned asset: a folder of files from one source page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    /// Folder name under `assets/third_party/`; unique across the lock.
    pub id: String,
    /// Display title, as the source names it.
    pub title: String,
    /// How the asset is used.
    pub kind: Kind,
    /// Human-readable source page.
    pub source: String,
    /// SPDX license identifier (`CC0-1.0`, `CC-BY-4.0`).
    pub license: String,
    /// Authors as credited by the source.
    pub authors: Vec<String>,
    /// Sample projects (folder names under `samples/`) that use it.
    pub projects: Vec<String>,
    /// Real-world size one texture repeat covers, in metres (textures only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meters: Option<[f32; 2]>,
    /// Files, relative to the asset folder.
    pub files: Vec<File>,
}

/// One downloaded file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct File {
    /// Path inside the asset folder (the source's own layout, so glTF URIs resolve).
    pub path: String,
    /// Stable download URL.
    pub url: String,
    /// Size in bytes.
    pub size: u64,
    /// BLAKE3 of the content, lowercase hex.
    pub blake3: String,
}

impl Lock {
    /// Reads and validates the manifest.
    pub fn read(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let lock: Self =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        lock.validate()?;
        Ok(lock)
    }

    /// Writes the manifest sorted by asset ID (and files by path).
    pub fn write(&mut self, path: &Path) -> Result<()> {
        self.assets.sort_by(|a, b| a.id.cmp(&b.id));
        for asset in &mut self.assets {
            asset.files.sort_by(|a, b| a.path.cmp(&b.path));
            asset.projects.sort();
            asset.projects.dedup();
        }
        self.validate()?;
        let text = ara_json::to_canonical_string(&ara_json::to_value(self)?);
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// The asset with this ID.
    pub fn get(&self, id: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Unique IDs, safe relative paths and well-formed hashes.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported lock version {}",
            self.version
        );
        let mut ids = std::collections::BTreeSet::new();
        for asset in &self.assets {
            ensure!(ids.insert(&asset.id), "duplicate asset {}", asset.id);
            ensure!(
                safe(&asset.id),
                "asset ID {} is not a safe folder name",
                asset.id
            );
            ensure!(!asset.license.is_empty(), "{}: license missing", asset.id);
            ensure!(!asset.files.is_empty(), "{}: no files", asset.id);
            for file in &asset.files {
                ensure!(
                    file.path.split('/').all(safe),
                    "{}: unsafe path {}",
                    asset.id,
                    file.path
                );
                ensure!(
                    file.blake3.len() == 64 && file.blake3.bytes().all(|b| b.is_ascii_hexdigit()),
                    "{}: bad hash for {}",
                    asset.id,
                    file.path
                );
            }
        }
        Ok(())
    }
}

impl Asset {
    /// The asset folder inside a project.
    pub fn dir(&self, project: &Path) -> PathBuf {
        project.join("assets/third_party").join(&self.id)
    }

    /// The project-relative path of one of its files (`assets/third_party/<id>/<path>`).
    pub fn relative(&self, file: &str) -> String {
        format!("assets/third_party/{}/{file}", self.id)
    }

    /// The first file with this extension (the model or HDR entry point).
    pub fn entry(&self, extensions: &[&str]) -> Option<&File> {
        self.files.iter().find(|f| {
            extensions
                .iter()
                .any(|e| f.path.to_ascii_lowercase().ends_with(e))
        })
    }
}

/// A non-empty path component without traversal or separators.
fn safe(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.contains(['\\', ':'])
        && !part.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, path: &str) -> Asset {
        Asset {
            id: id.into(),
            title: id.into(),
            kind: Kind::Model,
            source: "https://example.invalid".into(),
            license: "CC0-1.0".into(),
            authors: vec![],
            projects: vec!["outpost".into()],
            meters: None,
            files: vec![File {
                path: path.into(),
                url: "https://example.invalid/f".into(),
                size: 1,
                blake3: "0".repeat(64),
            }],
        }
    }

    #[test]
    fn duplicate_ids_and_traversal_are_rejected() {
        let ok = Lock {
            version: 1,
            assets: vec![asset("a", "textures/a.jpg"), asset("b", "b.gltf")],
        };
        assert!(ok.validate().is_ok());
        let duplicate = Lock {
            version: 1,
            assets: vec![asset("a", "a.gltf"), asset("a", "b.gltf")],
        };
        assert!(duplicate.validate().is_err());
        let escape = Lock {
            version: 1,
            assets: vec![asset("a", "../outside.bin")],
        };
        assert!(escape.validate().is_err());
    }
}
