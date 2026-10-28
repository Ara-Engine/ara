//! Published model bounds are decoded on one bounded worker and read as immutable metadata.
//! Navigation ticks only poll channels and publication counters; they never open asset files.
use ara_asset::Digest;
use ara_doc::{Workspace, prefab::Resolved};
use ara_play_host::{ImportProgress, PublishedAssets, PublishedSnapshot};
use glam::{Mat4, Vec3};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
mod data;
mod worker;
use data::LocalModel;
use worker::{Message, Worker};

/// A finite local or world axis-aligned bound, in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aabb {
    /// Minimum corner in the bound's coordinate space.
    pub min: Vec3,
    /// Maximum corner in the bound's coordinate space.
    pub max: Vec3,
}
impl Aabb {
    /// Ignore malformed/nonfinite bounds instead of poisoning camera matrices.
    pub fn new(min: Vec3, max: Vec3) -> Option<Self> {
        (min.is_finite() && max.is_finite() && min.cmple(max).all()).then_some(Self { min, max })
    }
    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
    /// Transform all corners, including mirrored/nonuniform instance scales.
    pub fn transformed(self, matrix: Mat4) -> Option<Self> {
        if !matrix.is_finite() {
            return None;
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for index in 0..8 {
            let point = matrix.transform_point3(Vec3::new(
                if index & 1 == 0 {
                    self.min.x
                } else {
                    self.max.x
                },
                if index & 2 == 0 {
                    self.min.y
                } else {
                    self.max.y
                },
                if index & 4 == 0 {
                    self.min.z
                } else {
                    self.max.z
                },
            ));
            min = min.min(point);
            max = max.max(point);
        }
        Self::new(min, max)
    }
}
struct Entry {
    digest: Digest,
    data: LocalModel,
}

/// The navigation service's small last-good bound cache. Vertex/texture payloads
/// live only on the worker while decoding; the UI keeps primitive boxes and selectors.
pub(crate) struct ModelBounds {
    reader: PublishedAssets,
    snapshot: Option<Arc<PublishedSnapshot>>,
    observed: u64,
    progress: Option<Arc<ImportProgress>>,
    progress_revision: u64,
    references: BTreeMap<String, String>,
    wanted: BTreeSet<String>,
    entries: BTreeMap<String, Entry>,
    failed: BTreeMap<String, Digest>,
    failures: Vec<String>,
    busy: bool,
    worker_failed: bool,
    worker: Worker,
}
impl ModelBounds {
    /// Attach once to the project's owner-published assets; no asset discovery or import occurs.
    pub fn new(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            reader: PublishedAssets::for_project(root).map_err(anyhow::Error::msg)?,
            snapshot: None,
            observed: u64::MAX,
            progress: None,
            progress_revision: u64::MAX,
            references: BTreeMap::new(),
            wanted: BTreeSet::new(),
            entries: BTreeMap::new(),
            failed: BTreeMap::new(),
            failures: Vec::new(),
            busy: false,
            worker_failed: false,
            worker: Worker::new()?,
        })
    }
    /// Update the references a changed scene uses. Call at document/publication boundaries,
    /// not on unchanged frames; ordinary transform edits reuse the same local metadata.
    pub fn track(&mut self, entities: &Resolved, workspace: &Workspace) {
        self.include(entities, workspace);
    }
    /// Refresh the bounded union of human/agent scene references at a document boundary.
    pub fn set_documents<'a>(
        &mut self,
        documents: impl IntoIterator<Item = &'a Resolved>,
        workspace: &Workspace,
    ) {
        self.wanted.clear();
        self.references.clear();
        for entities in documents {
            self.include(entities, workspace);
        }
        self.entries.retain(|asset, _| self.wanted.contains(asset));
        self.failed.retain(|asset, _| self.wanted.contains(asset));
    }
    fn include(&mut self, entities: &Resolved, workspace: &Workspace) {
        for entity in entities.values() {
            let Some(reference) = entity
                .record
                .get("MeshRenderer")
                .and_then(|renderer| renderer.get("model"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Ok(parsed) = ara_schema::grammar::parse_ref(reference) else {
                continue;
            };
            let asset = parsed
                .id
                .or_else(|| parsed.path.and_then(|path| workspace.index().id_of(path)));
            if let Some(asset) = asset {
                self.wanted.insert(asset.to_owned());
                self.references
                    .insert(reference.to_owned(), asset.to_owned());
            }
        }
    }
    /// Drain completed metadata and schedule at most one bounded decode batch.
    /// Returns true when usable bounds changed, so an explicit pending Frame can finish.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let message = match self.worker.results.try_recv() {
                Ok(message) => message,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if !self.worker_failed {
                        self.failures.push("Model bounds worker stopped; imported bounds retain their last good generation".into());
                        changed = true;
                    }
                    self.worker_failed = true;
                    self.busy = false;
                    break;
                }
            };
            match message {
                Message::Loaded {
                    asset,
                    digest,
                    result,
                } => {
                    if !self.wanted.contains(&asset) {
                        continue;
                    }
                    match result {
                        Ok(data) => {
                            self.entries.insert(asset.clone(), Entry { digest, data });
                            self.failed.remove(&asset);
                            changed = true;
                        }
                        Err(error) => {
                            self.failed.insert(asset.clone(), digest);
                            self.failures
                                .push(format!("Cannot frame imported model {asset}: {error}"));
                            changed = true;
                        }
                    }
                }
                Message::Done => {
                    self.busy = false;
                }
            }
        }
        let revision = self.reader.revision();
        if revision == 0 {
            self.observed = 0;
        }
        if revision != self.observed
            && let Some(snapshot) = self.reader.snapshot()
        {
            self.snapshot = Some(snapshot);
            self.observed = revision;
        }
        let progress = self.reader.progress_revision();
        if progress == 0 {
            self.progress_revision = 0;
        }
        if progress != self.progress_revision
            && let Some(state) = self.reader.progress()
        {
            self.progress = Some(state);
            self.progress_revision = progress;
            changed = true;
        }
        if !self.busy
            && !self.worker_failed
            && let Some(snapshot) = &self.snapshot
        {
            let assets: Vec<_> = self
                .wanted
                .iter()
                .filter_map(|asset| {
                    let digest = snapshot.generation(asset)?.digest();
                    (self
                        .entries
                        .get(asset)
                        .is_none_or(|entry| entry.digest != digest)
                        && self.failed.get(asset) != Some(&digest))
                    .then(|| (asset.to_owned(), digest))
                })
                .collect();
            if !assets.is_empty() {
                match self.worker.request(snapshot, assets) {
                    Ok(()) => self.busy = true,
                    Err(error) => {
                        self.worker_failed = true;
                        self.failures.push(error);
                        changed = true;
                    }
                }
            }
        }
        changed
    }
    /// Resolve a selected source node/primitive using its actual published vertices.
    /// A pending replacement keeps the preceding good metadata; no unit-box substitution.
    pub fn local(&self, renderer: &Value) -> Option<Aabb> {
        let reference = renderer.get("model")?.as_str()?;
        let asset = self.references.get(reference)?;
        self.entries.get(asset)?.data.selected(renderer)
    }
    /// Whether every imported model in the requested selection has metadata or a diagnosed failure.
    pub fn settled(&self, entities: &Resolved, selection: &[String]) -> bool {
        if self.worker_failed {
            return true;
        }
        entities
            .iter()
            .filter(|(id, _)| selection.is_empty() || selected(entities, id, selection))
            .all(|(_, entity)| {
                let Some(reference) = entity
                    .record
                    .get("MeshRenderer")
                    .and_then(|renderer| renderer.get("model"))
                    .and_then(Value::as_str)
                else {
                    return true;
                };
                let Some(asset) = self.references.get(reference) else {
                    return true;
                };
                if ara_schema::grammar::parse_ref(reference)
                    .ok()
                    .and_then(|reference| reference.path)
                    .is_some_and(|path| {
                        self.progress
                            .as_ref()
                            .is_some_and(|progress| progress.failed.contains_key(path))
                    })
                {
                    return true;
                }
                self.entries.contains_key(asset)
                    || self
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.generation(asset))
                        .is_some_and(|generation| {
                            self.failed.get(asset) == Some(&generation.digest())
                        })
            })
    }
    /// Publication counter for cheap navigation-service invalidation.
    pub fn revision(&self) -> u64 {
        self.reader.revision()
    }
    /// Background failures belong on the normal diagnostic bus, once per generation.
    pub fn take_failures(&mut self) -> Vec<String> {
        std::mem::take(&mut self.failures)
    }
}

/// A selected parent frames its descendants as one authored object.
pub(crate) fn selected(entities: &Resolved, id: &str, selection: &[String]) -> bool {
    let mut current = Some(id);
    for _ in 0..=entities.len() {
        let Some(id) = current else {
            return false;
        };
        if selection.iter().any(|selected| selected == id) {
            return true;
        }
        current = entities.get(id).and_then(|entity| entity.parent.as_deref());
    }
    false
}
