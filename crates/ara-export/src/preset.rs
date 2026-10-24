use anyhow::{Result, bail, ensure};
use ara_schema::{Schema, SchemaDb, ValueType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The one switch between the export conformance fixture and an engine runtime pin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayerTemplate {
    /// S7 ABI and pack verification only; does not execute authored demo gameplay.
    #[default]
    Conformance,
    /// The real engine player: a pinned ZIP with ara-player and cooked resources.
    Runtime,
}
impl PlayerTemplate {
    /// Versioned launch contract a pinned template must implement.
    pub fn contract(self) -> &'static str {
        match self {
            Self::Conformance => "ara.player.conformance/1",
            Self::Runtime => "ara.player.runtime/1",
        }
    }
}

/// Generic delivery format, independent of any upload service.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "camelCase")]
pub enum PackageFormat {
    /// Plain player folder and distribution manifest.
    #[default]
    Folder,
    /// Windows portable ZIP.
    WindowsPortable,
    /// Windows self-extracting executable and portable ZIP.
    WindowsInstaller,
    /// Linux type-2 AppImage using a separately pinned runtime.
    LinuxAppimage,
    /// Linux tar.gz with executable permissions.
    LinuxTar,
    /// Signed release Android APK and upload AAB, validated with bundletool.
    AndroidPlay,
}

/// Independently pinned tooling artifact, such as an installer stub or symbols.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ara(params)]
pub struct ArtifactPin {
    /// Expected BLAKE3, lowercase hex.
    pub hash: String,
    /// Local file or HTTPS source; cache hits are rehashed too.
    pub source: String,
}
impl ArtifactPin {
    /// Validates identity syntax before resolving any filesystem or network input.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.hash.len() == 64
                && self
                    .hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && !self.source.is_empty(),
            "ARA-EXP-TPL-002: invalid artifact pin"
        );
        Ok(())
    }
}

/// Optional Authenticode identity. Private keys remain in the Windows certificate store.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ara(params)]
pub struct WindowsSigning {
    /// Environment variable containing the signing certificate's SHA1 thumbprint.
    pub certificate_env: String,
    /// Optional HTTPS RFC3161 timestamp authority.
    pub timestamp_url: Option<String>,
}

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
    /// Matching native PDB for Windows templates; Linux templates contain DWARF.
    #[serde(default)]
    pub symbols: Option<ArtifactPin>,
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
    /// Selects the template contract; switching runtime pins is the engine integration seam.
    #[serde(default)]
    #[ara(default = "conformance")]
    pub player_template: PlayerTemplate,
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
    /// Generic delivery format, in addition to the plain build folder.
    #[serde(default)]
    #[ara(default = "folder")]
    pub packaging: PackageFormat,
    /// Local crash policy; uploads are disabled unless explicitly configured and consented.
    #[serde(default)]
    pub crash: ara_crash::CrashSettings,
    /// Prebuilt Windows installer executable, needed only by windows-installer.
    #[serde(default)]
    pub installer_stub: Option<ArtifactPin>,
    /// Type-2 AppImage runtime, needed only by linux-appimage.
    #[serde(default)]
    pub appimage_runtime: Option<ArtifactPin>,
    /// Optional Windows code-signing identity and timestamp endpoint.
    #[serde(default)]
    pub windows_signing: Option<WindowsSigning>,
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
        self.crash.validate()?;
        let compatible = match self.packaging {
            PackageFormat::Folder => true,
            PackageFormat::WindowsPortable | PackageFormat::WindowsInstaller => {
                self.platform == Platform::Windows
            }
            PackageFormat::LinuxAppimage | PackageFormat::LinuxTar => {
                self.platform == Platform::Linux
            }
            PackageFormat::AndroidPlay => {
                self.platform == Platform::Android
                    && self.signing.is_some()
                    && self.config == Configuration::Release
            }
        };
        ensure!(
            compatible,
            "ARA-EXP-DIST-001: package format does not match platform or requires Android release signing"
        );
        if self.packaging == PackageFormat::WindowsInstaller {
            ensure!(
                self.installer_stub.is_some(),
                "ARA-EXP-INSTALL-005: windows-installer requires a pinned installerStub"
            );
        }
        if self.packaging == PackageFormat::LinuxAppimage {
            ensure!(
                self.appimage_runtime.is_some(),
                "ARA-EXP-APPIMAGE-001: linux-appimage requires a pinned appimageRuntime"
            );
        }
        for pin in self
            .installer_stub
            .iter()
            .chain(self.appimage_runtime.iter())
            .chain(self.templates.iter().filter_map(|p| p.symbols.as_ref()))
        {
            pin.validate()?;
        }
        if let Some(signing) = &self.windows_signing {
            ensure!(
                self.platform == Platform::Windows
                    && !signing.certificate_env.is_empty()
                    && signing.timestamp_url.as_ref().is_none_or(
                        |url| url.starts_with("https://") && !url.contains(['\r', '\n'])
                    ),
                "ARA-EXP-SIGN-001: invalid Windows signing configuration"
            );
        }
        ensure!(
            !matches!(self.platform, Platform::Android | Platform::Ios)
                || self.lane == ScriptLane::NativeAot,
            "ARA-EXP-PRE-002: mobile requires NativeAOT"
        );
        ensure!(
            (1..=ara_pack::MAX_CHUNK as u32).contains(&self.chunk_size),
            "ARA-EXP-PRE-003: chunk size must be 1..=16 MiB"
        );
        let script_free =
            self.player_template == PlayerTemplate::Runtime && self.script_project.is_empty();
        ara_pack::validate_path(&self.content)?;
        if !script_free {
            ara_pack::validate_path(&self.script_project)?;
        }
        ensure!(
            script_free
                || (!self.assembly.is_empty()
                    && self
                        .assembly
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')),
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
