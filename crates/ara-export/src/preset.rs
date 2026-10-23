use anyhow::{Result, bail, ensure};
use ara_schema::{Schema, SchemaDb, ValueType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Destination operating system; Apple lanes delegate to an Apple silicon Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    /// Windows x64, Vulkan, NativeAOT or opt-in CoreCLR.
    Windows,
    /// Linux x64 with pinned glibc 2.31.
    Linux,
    /// Android arm64-v8a and x86_64.
    Android,
    /// Apple silicon macOS, built and signed on a Mac.
    Macos,
    /// Apple silicon iOS, built and signed on a Mac.
    Ios,
}
impl Platform {
    /// Stable lowercase platform name for artifacts and cook keys.
    pub fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Android => "android",
            Self::Macos => "macos",
            Self::Ios => "ios",
        }
    }
    /// Required runtime template architectures.
    pub fn arches(self) -> &'static [&'static str] {
        match self {
            Self::Android => &["arm64-v8a", "x86_64"],
            Self::Macos | Self::Ios => &["arm64"],
            _ => &["x64"],
        }
    }
}

/// C# shipping lane; only desktop permits opt-in ReadyToRun.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptLane {
    /// Ahead-of-time shared library, without JIT or runtime IL mods.
    #[default]
    NativeAot,
    /// Bundled CoreCLR with ReadyToRun assemblies and IL mod support.
    ReadyToRun,
}

/// Runtime configuration, included in template identity and cook keys.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "camelCase")]
pub enum Configuration {
    /// Development runtime with bridge enabled by launch configuration.
    Dev,
    /// Optimized runtime with bridge/profiling hooks.
    Profile,
    /// Shipping runtime with the bridge disabled.
    #[default]
    Release,
}
impl Configuration {
    /// Stable artifact configuration name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Profile => "profile",
            Self::Release => "release",
        }
    }
}

/// Hash-pinned runtime artifact for exactly one platform/architecture/configuration.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ara(params)]
pub struct TemplatePin {
    /// Destination OS.
    pub platform: Platform,
    /// Destination architecture.
    pub arch: String,
    /// Runtime configuration baked into this template.
    pub config: Configuration,
    /// BLAKE3 of the template file, lowercase hex.
    pub hash: String,
    /// Local file or HTTPS URL; read only when the content-addressed cache misses.
    pub source: String,
}

/// Android keystore configuration. Passwords are environment-variable references.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ara(params)]
pub struct AndroidSigning {
    /// Project-relative release/upload keystore.
    pub keystore: String,
    /// Keystore alias.
    pub alias: String,
    /// Environment variable containing the store password.
    pub store_password_env: String,
    /// Environment variable containing the private key password.
    pub key_password_env: String,
}

/// Optional native plugin; compilation is explicit rather than a template export default.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ara(params)]
pub struct NativePlugin {
    /// Project-relative Cargo manifest of a cdylib crate.
    pub manifest: String,
    /// Crate library stem, without prefix or extension.
    pub library: String,
}

/// Project-owned export document. Persist with `to_text`; never hand-order keys.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ara(params)]
pub struct ExportPreset {
    /// Destination OS.
    pub platform: Platform,
    /// Runtime configuration; release disables the observability server.
    #[serde(default)]
    #[ara(default = "release")]
    pub config: Configuration,
    /// NativeAOT by default; ReadyToRun is opt-in and desktop-only.
    #[serde(default)]
    #[ara(default = "nativeAot")]
    pub lane: ScriptLane,
    /// Project-relative C# project file.
    pub script_project: String,
    /// Assembly name and native image stem emitted by the C# project.
    pub assembly: String,
    /// Project-relative directory of cooked inputs/source JSON.
    pub content: String,
    /// Fixed raw delivery chunk size (default 1 MiB).
    #[serde(default = "default_chunk")]
    #[ara(default = 1048576)]
    pub chunk_size: u32,
    /// Runtime pins, one per destination architecture.
    pub templates: Vec<TemplatePin>,
    /// Explicit native plugins compiled in the native lane.
    #[serde(default)]
    pub plugins: Vec<NativePlugin>,
    /// Android application ID.
    #[serde(default = "default_package")]
    #[ara(default = "dev.ara.game")]
    pub package: String,
    /// Android minimum API; defaults to Android 10.
    #[serde(default = "default_min")]
    #[ara(default = 29)]
    pub min_api: u32,
    /// Android Play target API, configurable for future requirements.
    #[serde(default = "default_target")]
    #[ara(default = 36)]
    pub target_api: u32,
    /// Android version code, monotonically increasing for store uploads.
    #[serde(default = "default_version")]
    #[ara(default = 1)]
    pub version_code: u32,
    /// Plugin Android manifests whose uses-permission declarations are merged.
    #[serde(default)]
    pub permission_manifests: Vec<String>,
    /// Release/upload signing; absent means a generated debug key.
    #[serde(default)]
    pub signing: Option<AndroidSigning>,
    /// Environment variable containing a 64-character pack key (optional deterrent).
    #[serde(default)]
    pub encryption_key_env: Option<String>,
    /// Optional Mac-agent SSH destination; Apple builds require this seam.
    #[serde(default)]
    pub mac_agent: Option<String>,
}
fn default_chunk() -> u32 {
    1048576
}
fn default_package() -> String {
    "dev.ara.game".into()
}
fn default_min() -> u32 {
    29
}
fn default_target() -> u32 {
    36
}
fn default_version() -> u32 {
    1
}

impl ExportPreset {
    /// Registers this document and all nested types into the kernel Schema DB.
    pub fn register(db: &mut SchemaDb) {
        db.register::<Self>();
    }
    /// Parses strict JSON, validates schema identity and platform invariants.
    pub fn from_text(text: &str) -> Result<Self> {
        let mut value = ara_json::parse(text)?;
        ensure!(
            value.get("$schema").and_then(|v| v.as_str()) == Some("ara://schema/export/1"),
            "ARA-EXP-PRE-001: expected ara://schema/export/1"
        );
        value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("preset must be an object"))?
            .shift_remove("$schema");
        let preset: Self = serde_json::from_value(value)?;
        preset.validate()?;
        Ok(preset)
    }
    /// Writes schema-ordered canonical strict JSON with defaults omitted.
    pub fn to_text(&self) -> Result<String> {
        self.validate()?;
        let mut db = SchemaDb::new();
        Self::register(&mut db);
        let value = db.canonical_value(
            &ValueType::Named("ExportPreset".into()),
            &serde_json::to_value(self)?,
        );
        let mut obj = ara_json::Map::new();
        obj.insert("$schema".into(), "ara://schema/export/1".into());
        obj.extend(
            value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("schema canonical form is not an object"))?
                .clone(),
        );
        Ok(ara_json::to_canonical_string(&obj.into()))
    }
    /// Checks unsupported combinations and portable path/pin constraints.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !matches!(self.platform, Platform::Android | Platform::Ios)
                || self.lane == ScriptLane::NativeAot,
            "ARA-EXP-PRE-002: mobile requires NativeAOT"
        );
        ensure!(
            (1..=ara_pack::MAX_CHUNK as u32).contains(&self.chunk_size),
            "ARA-EXP-PRE-003: chunk size must be 1..=16 MiB"
        );
        for p in [&self.script_project, &self.content] {
            ara_pack::validate_path(p)?;
        }
        ensure!(
            !self.assembly.is_empty()
                && self
                    .assembly
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "ARA-EXP-PRE-004: invalid assembly stem"
        );
        if self.platform == Platform::Android {
            ensure!(
                self.min_api >= 29
                    && self.target_api >= 36
                    && self.target_api >= self.min_api
                    && self.version_code > 0,
                "ARA-EXP-AND-013: Android requires min >=29, target >=36 and version_code >0"
            );
            ensure!(
                self.package.split('.').count() >= 2
                    && self
                        .package
                        .split('.')
                        .all(|p| p.starts_with(|c: char| c.is_ascii_alphabetic())
                            && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')),
                "ARA-EXP-AND-014: invalid package ID"
            );
        }
        let mut arches = BTreeSet::new();
        for pin in &self.templates {
            ensure!(
                pin.platform == self.platform
                    && pin.config == self.config
                    && self.platform.arches().contains(&pin.arch.as_str())
                    && arches.insert(&pin.arch),
                "ARA-EXP-TPL-001: mismatched or duplicate template tuple"
            );
            ensure!(
                pin.hash.len() == 64
                    && pin
                        .hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "ARA-EXP-TPL-002: invalid template hash"
            );
        }
        if !matches!(self.platform, Platform::Macos | Platform::Ios)
            && arches.len() != self.platform.arches().len()
        {
            bail!("ARA-EXP-TPL-003: one pinned template per architecture is required");
        }
        for p in &self.permission_manifests {
            ara_pack::validate_path(p)?;
        }
        for p in &self.plugins {
            ara_pack::validate_path(&p.manifest)?;
            ensure!(
                !p.library.is_empty()
                    && p.library
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "ARA-EXP-PRE-005: invalid plugin library stem"
            );
        }
        if let Some(s) = &self.signing {
            ara_pack::validate_path(&s.keystore)?;
            ensure!(
                !s.alias.is_empty(),
                "ARA-EXP-AND-015: signing alias is empty"
            );
        }
        Ok(())
    }
}
