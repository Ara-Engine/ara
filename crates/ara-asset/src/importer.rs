use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// BLAKE3 digest of canonical inputs or exact artifact bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Digest(pub [u8; 32]);
impl Digest {
    /// Hashes exact bytes.
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }
}
impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// Explicit cook profile; desktop targets share the BCn encoding family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetPlatform {
    /// Windows Vulkan profile.
    Windows,
    /// Linux Vulkan profile.
    Linux,
    /// Apple silicon desktop profile (BC support checked by runtime).
    MacOs,
    /// Mobile profile; requires a registered ASTC encoder.
    Android,
    /// Apple mobile profile; requires a registered ASTC encoder.
    Ios,
}
/// A frozen input snapshot; importers never read files or the wall clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportInput {
    /// Encoder preset, included in the action digest. Shipping defaults to HQ.
    #[serde(default)]
    pub preset: crate::CookPreset,
    /// Maximum Rayon threads for this job (1..=256); excluded from cache identity.
    #[serde(default = "crate::preset::default_thread_budget")]
    pub thread_budget: u16,
    /// Relative source path, also part of the cache key.
    pub source_path: String,
    /// Exact source media bytes.
    #[serde(with = "serde_bytes")]
    pub source: Vec<u8>,
    /// Stable importer ID.
    pub importer: String,
    /// Implementation/settings ABI version, including encoder versions.
    pub version: String,
    /// Declarative settings (canonical key order is independent of insertion).
    pub settings: serde_json::Value,
    /// Platform-specific cook profile.
    pub target: TargetPlatform,
    /// Exact external dependencies, keyed by normalized URI relative to source.
    pub dependencies: BTreeMap<String, Vec<u8>>,
    /// Content identities for referenced assets whose cooked output is an input.
    pub asset_dependencies: BTreeMap<String, Digest>,
}
impl ImportInput {
    /// Action digest covers semantic inputs and dependency hashes, length-framed.
    /// Thread budgets affect scheduling only and never fragment the cache.
    pub fn key(&self) -> Result<Digest> {
        let mut h = blake3::Hasher::new_derive_key("Ara import action v2");
        let mut add = |b: &[u8]| {
            h.update(&(b.len() as u64).to_le_bytes());
            h.update(b);
        };
        for s in [&self.source_path, &self.importer, &self.version] {
            add(s.as_bytes());
        }
        add(&Digest::of(&self.source).0);
        add(ara_json::to_canonical_string(&ara_json::sort_keys(&self.settings)).as_bytes());
        add(&serde_json::to_vec(&self.target)?);
        add(&serde_json::to_vec(&self.preset)?);
        add(&(self.dependencies.len() as u64).to_le_bytes());
        for (path, bytes) in &self.dependencies {
            add(path.as_bytes());
            add(&Digest::of(bytes).0);
        }
        add(b"asset-dependencies");
        add(&(self.asset_dependencies.len() as u64).to_le_bytes());
        for (id, digest) in &self.asset_dependencies {
            add(id.as_bytes());
            add(&digest.0);
        }
        Ok(Digest(*h.finalize().as_bytes()))
    }
}
/// Semantic payload type for cache validation and runtime dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    /// ARAF mesh v1.
    Mesh,
    /// GPU compressed KTX2.
    Texture,
    /// ARAF skeleton v1.
    Skeleton,
    /// ARAF clip v1.
    Clip,
    /// Canonical JSON scene/material projection.
    Scene,
    /// Plugin-defined payload; plugin owns validation.
    Custom,
}
/// One deterministic named result of a cook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Stable subasset name, independent of machine paths.
    pub name: String,
    /// Cooked payload schema.
    pub kind: ArtifactKind,
    /// Exact immutable bytes.
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}
impl Artifact {
    /// Validates the payload before cache insertion/publication.
    pub fn validate(&self) -> Result<()> {
        match self.kind {
            ArtifactKind::Mesh => {
                ara_asset_format::Mesh::read(&self.bytes)?;
            }
            ArtifactKind::Texture => {
                ara_asset_format::Texture::read(&self.bytes)?;
            }
            ArtifactKind::Skeleton => {
                ara_asset_format::Skeleton::read(&self.bytes)?;
            }
            ArtifactKind::Clip => {
                ara_asset_format::Clip::read(&self.bytes)?;
            }
            ArtifactKind::Scene => {
                let text =
                    std::str::from_utf8(&self.bytes).map_err(|e| Error::Invalid(e.to_string()))?;
                ara_json::parse(text).map_err(|e| Error::Invalid(e.to_string()))?;
            }
            ArtifactKind::Custom => {}
        }
        Ok(())
    }
}
/// Complete output, sorted by stable subasset name before persistence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportOutput {
    /// Meshes, textures, skeletons, clips and a scene manifest.
    pub artifacts: Vec<Artifact>,
}
impl ImportOutput {
    /// Enforces unique nonempty artifact names and validates every payload.
    pub fn validate(&self) -> Result<()> {
        let mut names = std::collections::BTreeSet::new();
        if self.artifacts.is_empty() {
            return Err(Error::Invalid("import produced no artifacts".into()));
        }
        for a in &self.artifacts {
            if a.name.is_empty() || !names.insert(&a.name) {
                return Err(Error::Invalid("duplicate/empty artifact name".into()));
            }
            a.validate()?;
        }
        Ok(())
    }
}
/// Cooperative cancellation token; process jobs additionally kill native encoders.
#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    /// Cancels every clone of this token.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// Checks whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// Stops a cooperative importer at a work boundary.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
/// Protocol-ready progress update (does not participate in deterministic output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    /// Completed fraction in [0,1].
    pub fraction: f32,
    /// Short stage description.
    pub message: String,
}
/// Worker-local progress and cancellation hooks.
pub struct ImportContext<'a> {
    /// Optional per-job profiling sink; timers never enter cooked data.
    pub metrics: Option<&'a crate::ImportMetrics>,
    /// Cancellation requested by the caller.
    pub cancellation: &'a Cancellation,
    /// Invoked only between work units; never affects cook bytes.
    pub progress: &'a mut (dyn FnMut(Progress) + Send),
}
impl ImportContext<'_> {
    /// Reports finite clamped progress and checks cancellation.
    pub fn report(&mut self, fraction: f32, message: impl Into<String>) -> Result<()> {
        self.cancellation.check()?;
        (self.progress)(Progress {
            fraction: if fraction.is_finite() {
                fraction.clamp(0.0, 1.0)
            } else {
                0.0
            },
            message: message.into(),
        });
        Ok(())
    }
}
/// Pure plugin importer. Implementations are installed in the worker executable.
pub trait Importer: Send + Sync {
    /// Stable plugin ID (for example `ara.gltf`).
    fn id(&self) -> &str;
    /// Version of all algorithms that can change output bytes.
    fn version(&self) -> &str;
    /// Registers declarative settings metadata without parsing source content.
    /// Importers without configurable settings can keep the default implementation.
    fn register_settings(&self, _db: &mut ara_schema::SchemaDb) {}
    /// Discovers external input URIs from source bytes without doing I/O.
    fn dependencies(&self, _source: &[u8]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// Cooks only the supplied snapshot, with no environment-dependent state.
    fn import(&self, input: &ImportInput, context: &mut ImportContext<'_>) -> Result<ImportOutput>;
}
/// Explicit importer installation; no dynamic code loading in the editor process.
#[derive(Default)]
pub struct ImporterRegistry {
    importers: BTreeMap<String, Box<dyn Importer>>,
}
impl ImporterRegistry {
    /// Registers settings types for generated inspectors, docs and protocol schemas.
    pub fn register_schemas(&self, db: &mut ara_schema::SchemaDb) {
        for importer in self.importers.values() {
            importer.register_settings(db);
        }
    }
    /// Adds an importer, rejecting ID clashes.
    pub fn register(&mut self, importer: impl Importer + 'static) -> Result<()> {
        let id = importer.id().to_owned();
        if self.importers.contains_key(&id) {
            return Err(Error::Invalid(format!("duplicate importer {id}")));
        }
        self.importers.insert(id, Box::new(importer));
        Ok(())
    }
    /// Looks up a plugin for dependency discovery or worker execution.
    pub fn get(&self, id: &str) -> Result<&dyn Importer> {
        self.importers
            .get(id)
            .map(Box::as_ref)
            .ok_or_else(|| Error::Invalid(format!("unknown importer {id}")))
    }
    /// Runs an exact preset/version in a private bounded pool and validates it.
    /// Editor callers execute this through ProcessWorker, never on a frame thread.
    pub fn import(&self, input: &ImportInput, ctx: &mut ImportContext<'_>) -> Result<ImportOutput> {
        let importer = self.get(&input.importer)?;
        if importer.version() != input.version {
            return Err(Error::Invalid("importer version mismatch".into()));
        }
        ctx.report(0.0, "starting")?;
        if !(1..=256).contains(&input.thread_budget) {
            return Err(Error::Invalid("thread budget must be in 1..=256".into()));
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(usize::from(input.thread_budget))
            .thread_name(|i| format!("ara-import-{i}"))
            .build()
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let mut output = pool.install(|| importer.import(input, ctx))?;
        output.artifacts.sort_by(|a, b| a.name.cmp(&b.name));
        {
            let _timer = ctx.metrics.map(|m| m.start(crate::ImportStage::Validation));
            output.validate()?;
        }
        ctx.report(1.0, "ready")?;
        Ok(output)
    }
}
