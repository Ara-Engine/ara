//! Background preparation for OS-file imports. Authored files change only when
//! the host commits the returned immutable states through its typed transaction.
mod external;
mod planner;
mod preflight;
mod prepare;

use ara_asset::{
    Cancellation, Digest, Error, ImportInput, ImportOutput, ProcessWorker, Result, SourceAsset,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub use preflight::{preflight, run};
pub use prepare::{prepare, read_entry};

/// How an incoming entry colliding with an existing destination is handled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportConflict {
    /// Allocate a deterministic unused name; existing data remains untouched.
    #[default]
    Rename,
    /// Replace matching file kinds, retaining destination asset identity/settings;
    /// existing directories merge without deleting unmentioned children.
    Replace,
    /// Skip a colliding file or complete directory subtree.
    Skip,
}

/// User intent; paths are frozen before worker preparation starts.
pub struct Request {
    /// Explicit absolute OS files or folder roots authorizing external reads.
    pub sources: Vec<PathBuf>,
    /// Authored-relative destination folder; empty means the authored root.
    pub destination: String,
    /// Explicit collision policy; default Rename is nondestructive.
    pub conflict: ImportConflict,
    /// Read-only conflict/source preflight; never stages, cooks or publishes files.
    pub dry_run: bool,
}

/// Actual collision at the requested path, retained after conflict resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// Incoming and existing regular file.
    File,
    /// Incoming and existing directory.
    Directory,
    /// A file and directory cannot be merged/replaced as the same kind.
    TypeMismatch,
    /// A media destination is occupied through its paired/orphan import sidecar.
    Sidecar,
}
impl ConflictKind {
    /// Exact snake_case wire enum consumed by file-drop clients.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::TypeMismatch => "type_mismatch",
            Self::Sidecar => "sidecar",
        }
    }
}

/// A real pre-policy collision; never inferred from renamed/skipped outcomes.
#[derive(Clone, Debug)]
pub struct Conflict {
    /// Absolute selected external source.
    pub source: PathBuf,
    /// Requested authored-relative collision path, before conflict renaming.
    pub destination_logical_path: String,
    /// Existing destination asset identity, or None for unowned files/directories.
    pub existing_asset_id: Option<String>,
    /// Exact collision category.
    pub kind: ConflictKind,
}

/// Read-only validated plan; it owns no staging directory or mutation capability.
pub struct ImportPlan {
    /// Complete current destination freshness token.
    pub token: String,
    /// Actual collisions, including those resolved by Rename/Replace/Skip.
    pub conflicts: Vec<Conflict>,
    /// Final deterministic metadata plan after the selected conflict policy.
    pub planned: Vec<Entry>,
    /// Estimated peak disk requirement for executing this plan.
    pub estimated_required_bytes: u64,
}

/// One background entry point for the same typed request in both modes.
pub enum Outcome {
    /// Read-only preflight result, with no staged/cooked files.
    Preflight(ImportPlan),
    /// Fully staged/cooked import, still cancellable before typed atomic commit.
    Ready(PreparedImport),
}

/// Entry kind recorded by the complete destination inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file, never a link/reparse point.
    File,
    /// A directory, including empty authored folders.
    Directory,
}

/// Existing destination identity supplied by the host, not reread by the planner.
#[derive(Clone, Debug)]
pub struct ExistingEntry {
    /// Existing filesystem kind.
    pub kind: EntryKind,
    /// Logical length reserved conservatively for a replacement/undo snapshot copy.
    pub bytes: u64,
    /// BLAKE3 byte proof for files (not the shared FileState SHA-256 token).
    pub digest: Option<Digest>,
    /// Native document/sidecar identity retained by Replace.
    pub asset_id: Option<String>,
    /// Existing source binding, including sidecar settings and effective preset.
    pub binding: Option<SourceAsset>,
}

/// Complete, link-free authored inventory bound to one host freshness token.
pub struct DestinationInventory {
    /// Includes source/sidecar/policy and directory-state identity.
    pub token: String,
    /// Actual destination filesystem's case behavior.
    pub case_insensitive: bool,
    /// Canonical authored-relative files and directories.
    pub entries: BTreeMap<String, ExistingEntry>,
}

/// Explicit resource budgets checked before staging and again before cooking.
pub struct Limits {
    /// Maximum sum of incoming file lengths.
    pub bytes: u64,
    /// Maximum number of incoming files/directories.
    pub entries: usize,
}

/// Resolves importer capabilities/settings without parsing native source formats.
pub trait Importers: Send + Sync {
    /// Freeze a final-plan resolver before discovery/cooking. Native/sidecar IDs
    /// and same-selection authoring dependencies can be projected here without
    /// writing files. Called again after URI-preserving Rename regrouping.
    fn prepare_plan(
        &self,
        _entries: &[Entry],
        _read: &dyn Fn(&Entry) -> Result<Vec<u8>>,
        _cancel: &Cancellation,
    ) -> Result<Option<Box<dyn Importers>>> {
        Ok(None)
    }
    /// Refresh existing version/dependency proofs for Replace while preserving
    /// destination identity and author settings.
    fn rebind(&self, binding: &SourceAsset) -> Result<SourceAsset> {
        Ok(binding.clone())
    }
    /// New source binding; return None for a non-source/native Ara document.
    /// `foreign_sidecar` is allowed only from explicitly selected files/folders;
    /// its ID must be remapped at commit, never copied blindly. Resolve inherited
    /// presets before returning. Replace uses its existing binding instead.
    fn source(
        &self,
        destination: &str,
        foreign_sidecar: Option<&[u8]>,
    ) -> Result<Option<SourceAsset>>;
    /// Whether the host's schema/UI adapter recognizes this native document path.
    fn native_document(&self, destination: &str) -> bool;
}

/// Disposable-worker operations, separable from filesystem preparation in tests.
pub trait SourceWorker: Send + Sync {
    /// Discover local inputs using importer/settings metadata out of process.
    fn discover(
        &self,
        binding: &SourceAsset,
        source: Vec<u8>,
        cancel: &Cancellation,
    ) -> Result<Vec<String>>;
    /// Cook/validate an immutable input before any authored publication.
    fn cook(
        &self,
        input: ImportInput,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(ara_asset::Progress),
    ) -> Result<ImportOutput>;
}
impl SourceWorker for ProcessWorker {
    fn discover(
        &self,
        binding: &SourceAsset,
        source: Vec<u8>,
        cancel: &Cancellation,
    ) -> Result<Vec<String>> {
        self.discover_with_settings(
            &binding.importer,
            &binding.version,
            source,
            binding.settings.clone(),
            cancel,
        )
    }
    fn cook(
        &self,
        input: ImportInput,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(ara_asset::Progress),
    ) -> Result<ImportOutput> {
        let mut job = self.start(input)?;
        loop {
            cancel.check()?;
            match job.poll()? {
                Some(ara_asset::JobEvent::Progress(update)) => progress(update),
                Some(ara_asset::JobEvent::Complete(output)) => {
                    output.validate()?;
                    return Ok(output);
                }
                Some(_) => {
                    return Err(Error::Worker(
                        "unexpected importer reply while preparing file drop".into(),
                    ));
                }
                None => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        }
    }
}

/// Background-only dependencies; the host owns profile isolation and space checks.
pub struct Context<'a> {
    /// Canonical project root; legacy layouts may use this as the authored root.
    pub project_root: &'a Path,
    /// Actual authored root; preparation never writes inside it.
    pub authored_root: &'a Path,
    /// Fresh absolute staging directory with an existing parent, outside authored data.
    pub staging: &'a Path,
    /// Immutable destination view used by the pure collision planner.
    pub inventory: &'a DestinationInventory,
    /// Existing disposable importer process implementation.
    pub worker: &'a dyn SourceWorker,
    /// Registration/schema adapter, including existing plugin importers.
    pub importers: &'a dyn Importers,
    /// Admission budgets; zero is not an unlimited sentinel.
    pub limits: Limits,
    /// Checks estimated peak disk space at `staging`; label it an estimate in UI.
    /// Includes staging, incoming immutable copies, replacement snapshots and
    /// cooked output allowance. Use the host's platform free-space helper.
    pub check_space: &'a (dyn Fn(&Path, u64) -> Result<()> + Send + Sync),
}

/// Preparation phase; the commit transition is owned by the typed job authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Enumerating external metadata and planning conflicts.
    Inspecting,
    /// Copying authorized bytes into immutable staging.
    Staging,
    /// Discovering companion URIs in the disposable worker.
    Discovering,
    /// Validating/cooking prepared source inputs out of process.
    Cooking,
    /// Complete and still cancellable until the authority starts atomic commit.
    Ready,
}

/// Metadata/byte progress suitable for a cancellable asynchronous job.
pub struct Progress {
    /// Current phase.
    pub phase: Phase,
    /// Staged bytes, never greater than total.
    pub received: u64,
    /// Total admitted bytes after conflict planning.
    pub total: u64,
    /// Number of inspected/staged entries.
    pub entries: usize,
    /// Current authored-relative destination, when known.
    pub path: Option<String>,
}

/// Result of an explicit conflict decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// A fresh entry at its requested path.
    Created,
    /// An existing regular file is replaced, with identity/settings retained.
    Replaced,
    /// This entry/subtree is left untouched.
    Skipped,
    /// A fresh entry received an unused deterministic name.
    Renamed,
    /// Existing directory retained while explicitly merging incoming children.
    Merged,
}

/// One deterministic prepared file/directory operation.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Exact original logical spelling from the common authorized selection root.
    pub source_reference: String,
    /// Absolute original source, or None for a new destination parent directory.
    pub source: Option<PathBuf>,
    /// Authored-relative final path.
    pub destination: String,
    /// Kind of the incoming entry.
    pub kind: EntryKind,
    /// Planned conflict outcome.
    pub disposition: Disposition,
    /// Exact admitted file length (zero for directories).
    pub bytes: u64,
    /// BLAKE3 proof of admitted bytes; separate from shared SHA-256 file history.
    pub digest: Option<Digest>,
    /// Staged regular file, never an authored destination.
    pub staged: Option<PathBuf>,
    /// Existing identity retained by Replace.
    pub asset_id: Option<String>,
    /// Native documents must be parsed/remapped through the Ara document adapter
    /// before commit; these bytes must not be installed with foreign IDs intact.
    pub native_document: bool,
}

/// Out-of-process source validation completed before authored publication.
pub struct PreparedSource {
    /// Authored-relative final consumer path.
    pub path: String,
    /// New binding template or unchanged destination binding for Replace.
    pub binding: SourceAsset,
    /// Exact source/dependency bytes consumed by the disposable worker.
    pub input: ImportInput,
    /// Validated artifacts, ready for derived-cache insertion by the host.
    pub output: ImportOutput,
    /// Incoming sidecar bytes for ID remapping; Replace keeps destination settings.
    pub foreign_sidecar: Option<Vec<u8>>,
}

/// Owns immutable staging until commit or explicit discard. Dropping it never
/// changes authored data; only its own fresh staging directory is removed.
pub struct PreparedImport {
    /// Destination inventory token that must still match at commit.
    pub token: String,
    /// Planned operations, in canonical destination order.
    pub entries: Vec<Entry>,
    /// Validated/cooked source snapshots.
    pub sources: Vec<PreparedSource>,
    /// Conservative peak disk requirement, an estimate rather than exact allocation.
    pub estimated_required_bytes: u64,
    /// Real pre-policy collisions retained for consistent preview/execution reports.
    pub conflicts: Vec<Conflict>,
    staging: prepare::Staging,
}
impl PreparedImport {
    /// Revalidates staged bytes before entering the brief noncancellable commit.
    pub fn verify(&self, cancellation: &Cancellation) -> Result<()> {
        prepare::verify(self, cancellation)
    }
    /// Explicit cancellation cleanup; reports a retained staging directory on failure.
    pub fn discard(mut self) -> Result<()> {
        self.staging.discard()
    }
}

#[cfg(test)]
mod tests;
