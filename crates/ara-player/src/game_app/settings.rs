//! Presentation preferences; saved outside authored game documents.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Settings {
    pub quality: usize,
    pub resolution: usize,
    pub window_mode: usize,
    pub master: f32,
    pub music: f32,
    pub sfx: f32,
    pub sensitivity: f32,
    pub invert: bool,
}
impl Default for Settings {
    fn default() -> Self { Self {quality:2,resolution:0,window_mode:0,master:0.8,music:0.45,sfx:0.8,sensitivity:1.,invert:false} }
}
impl Settings {
    pub(super) const QUALITIES: [&'static str;4] = ["low","medium","high","epic"];
    pub(super) const SIZES: [[u32;2];3] = [[1280,720],[1920,1080],[2560,1440]];
    fn path() -> PathBuf {
        std::env::var_os("ARA_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".ara/config"))
            .join("outpost-settings.json")
    }
    pub(super) fn load() -> Result<Self> {
        let result = match std::fs::read(Self::path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Outpost settings")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(result)
    }
    pub(super) fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(path, ara_json::to_canonical_string(&serde_json::to_value(self)?))?;
        Ok(())
    }
    pub(super) fn quality(&self) -> ara_runtime_render::QualityChoice {
        Self::QUALITIES[self.quality % 4].parse().expect("the four engine quality names are valid")
    }
    pub(super) fn size(&self) -> [u32;2] { Self::SIZES[self.resolution % 3] }
}
