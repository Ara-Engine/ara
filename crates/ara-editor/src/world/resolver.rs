//! The open scene's prefab resolution kept current across transactions.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ara_diag::{Diagnostic, Location};
use ara_doc::prefab::{Resolved, check_parents, resolve_record_reporting};
use ara_doc::{AssetId, Document, Op, RecordId};
use ara_ops::Store;

/// [`super::resolve`] for a scene, kept current across transactions. A new revision
/// splices in only the scene records its transactions touched (undo and redo are
/// transactions too), each resolved alone (`prefab::resolve_record_reporting`):
/// records resolve independently, and the one check across records
/// (`prefab::check_parents`) runs again over the result, so the answer is exactly a
/// full resolve's, problems included. Anything else resolves in full: an op on
/// another document (a prefab edited), a document loaded or unloaded
/// (`Workspace::generation`), another scene, a gap in the history.
///
/// It keeps two resolutions and advances the one no caller still holds, so a caller
/// keeping the previous one (the view the Scene diffs a commit against) costs no copy.
#[derive(Default)]
pub struct SceneResolver {
    buffers: Vec<Buffer>,
    unavailable: Arc<Resolved>,
}

/// One scene's resolution at one revision.
struct Buffer {
    doc: AssetId,
    revision: u64,
    generation: u64,
    entities: Arc<Resolved>,
    /// Each record's problems, as resolving it reported them.
    problems: BTreeMap<RecordId, Vec<Diagnostic>>,
    /// Every problem (records' in record order, then the parents check's) when any is
    /// an error: `resolve` fails with them.
    errors: Option<Vec<Diagnostic>>,
}

impl Buffer {
    /// Every record of `document`, resolved as `prefab::resolve` does.
    fn full(store: &Store, doc: AssetId, document: &Document, revision: u64) -> Self {
        let mut entities = Resolved::new();
        let mut problems = BTreeMap::new();
        for record in document.records().keys() {
            let mut found = Vec::new();
            if let Some(part) = resolve_record(store, document, record, &mut found) {
                entities.extend(part);
            }
            if !found.is_empty() {
                problems.insert(record.clone(), found);
            }
        }
        let mut buffer = Buffer {
            doc,
            revision,
            generation: store.workspace().generation(),
            entities: Arc::new(entities),
            problems,
            errors: None,
        };
        buffer.check(document);
        buffer
    }
    /// Resolve `records` of `document` again (dropping those that no longer exist),
    /// then the check across records.
    fn splice(&mut self, store: &Store, document: &Document, records: &BTreeSet<RecordId>) {
        let entities = Arc::make_mut(&mut self.entities);
        for record in records {
            let prefix = format!("{record}/");
            entities.retain(|addr, _| addr != record.as_str() && !addr.starts_with(&prefix));
            self.problems.remove(record);
            let mut found = Vec::new();
            if let Some(part) = resolve_record(store, document, record, &mut found) {
                entities.extend(part);
            }
            if !found.is_empty() {
                self.problems.insert(record.clone(), found);
            }
        }
        self.check(document);
    }
    fn check(&mut self, document: &Document) {
        let mut all: Vec<Diagnostic> = self.problems.values().flatten().cloned().collect();
        check_parents(document, &self.entities, &Location::default(), &mut all);
        self.errors = ara_diag::has_errors(&all).then_some(all);
    }
    fn result(&self) -> Result<Arc<Resolved>, Vec<Diagnostic>> {
        match &self.errors {
            Some(errors) => Err(errors.clone()),
            None => Ok(Arc::clone(&self.entities)),
        }
    }
    fn held(&self) -> bool {
        Arc::strong_count(&self.entities) > 1
    }
}

impl SceneResolver {
    /// Resolve the active scene for display, retaining an empty view while its
    /// document is absent. Reappearing documents use normal revision/generation
    /// invalidation; authoring callers must continue to use [`Self::resolve`].
    pub fn presentation(
        &mut self,
        store: &Store,
        doc: &str,
    ) -> Result<Arc<Resolved>, Vec<Diagnostic>> {
        match self.resolve(store, doc) {
            Err(_) if !super::document_available(store, doc) => Ok(Arc::clone(&self.unavailable)),
            result => result,
        }
    }
    /// `world::resolve(store, doc)`, reusing earlier resolutions where it can.
    pub fn resolve(&mut self, store: &Store, doc: &str) -> Result<Arc<Resolved>, Vec<Diagnostic>> {
        let ws = store.workspace();
        let Some((id, document)) = ws
            .find(doc)
            .and_then(|id| Some((id.clone(), ws.doc(id.as_str())?)))
        else {
            return super::resolve(store, doc).map(Arc::new);
        };
        let (revision, generation) = (store.revision(), ws.generation());
        if let Some(buffer) = self
            .buffers
            .iter()
            .find(|b| b.doc == id && b.revision == revision && b.generation == generation)
        {
            return buffer.result();
        }
        // What each buffer would resolve again to get here; a buffer splicing cannot
        // bring here (another scene, a document loaded or changed) is dropped.
        let mut pending = Vec::new();
        self.buffers.retain(|b| {
            let records = (b.doc == id && b.generation == generation)
                .then(|| touched(store, &id, b.revision, revision))
                .flatten();
            pending.extend(records.clone());
            records.is_some()
        });
        // A buffer no caller holds first (spliced in place), then the newest.
        let best = (0..self.buffers.len())
            .max_by_key(|&i| (!self.buffers[i].held(), self.buffers[i].revision));
        let index = match best {
            // Every buffer is held: advance a copy, kept beside the held one, which
            // the next revision can then advance in place once released.
            Some(i) if self.buffers[i].held() && self.buffers.len() < 2 => {
                let held = &self.buffers[i];
                let mut copy = Buffer {
                    doc: id,
                    revision,
                    generation,
                    entities: Arc::new((*held.entities).clone()),
                    problems: held.problems.clone(),
                    errors: None,
                };
                copy.splice(store, document, &pending[i]);
                self.buffers.push(copy);
                self.buffers.len() - 1
            }
            Some(i) => {
                let buffer = &mut self.buffers[i];
                buffer.revision = revision;
                buffer.splice(store, document, &pending[i]);
                i
            }
            None => {
                self.buffers
                    .push(Buffer::full(store, id, document, revision));
                self.buffers.len() - 1
            }
        };
        self.buffers[index].result()
    }
}

/// The scene records the transactions from revision `from` to `to` touched, or None
/// when splicing cannot follow them: an op on another document or a document's
/// lifecycle, or a gap in the history.
fn touched(store: &Store, doc: &AssetId, from: u64, to: u64) -> Option<BTreeSet<RecordId>> {
    let since = store.since(from);
    if from > to || since.len() as u64 != to - from {
        return None;
    }
    let mut records = BTreeSet::new();
    for op in since.iter().flat_map(|txn| &txn.ops) {
        if op.doc() != doc || matches!(op, Op::Document { .. }) {
            return None;
        }
        records.insert(op.id().clone());
    }
    Some(records)
}

/// One record's entities and problems, as `prefab::resolve` has them.
fn resolve_record(
    store: &Store,
    document: &Document,
    record: &RecordId,
    found: &mut Vec<Diagnostic>,
) -> Option<Resolved> {
    resolve_record_reporting(
        document,
        record,
        store.schema(),
        store.workspace(),
        &Location::default(),
        Some(found),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_doc::{AssetIndex, Edit, Record, Workspace, read};
    use ara_ops::{Author, TxnMeta};
    use ara_schema::{FieldPath, SchemaDb};
    use serde_json::{Value, json};

    const CRATE: &str = r#"{ "$schema": "ara://schema/prefab/1", "id": "pf_crate001", "records": {
        "e_crt1": { "name": "Crate", "Transform": {} },
        "e_lid1": { "name": "Lid", "parent": "e_crt1", "Transform": {} } } }"#;
    const STACK: &str = r#"{ "$schema": "ara://schema/prefab/1", "id": "pf_stack001", "records": {
        "e_stk1": { "name": "Stack", "Transform": {} },
        "e_c001": { "parent": "e_stk1", "prefab": "@prefabs/crate.prefab#pf_crate001" },
        "e_c002": { "parent": "e_stk1", "prefab": "@prefabs/crate.prefab#pf_crate001",
            "overrides": { "e_lid1/Transform.scale": [3, 3, 3] } } } }"#;
    const SCENE: &str = r#"{ "$schema": "ara://schema/scene/1", "id": "sc_level001", "records": {
        "e_lamp1": { "name": "Lamp", "Transform": {} },
        "e_kid1": { "parent": "e_lamp1", "Transform": {} },
        "e_box1": { "prefab": "@prefabs/crate.prefab#pf_crate001", "name": "Box",
            "overrides": { "Transform.position": [1, 0, 0] } },
        "e_pile1": { "prefab": "@prefabs/stack.prefab#pf_stack001", "removed": ["e_c001/e_lid1"] },
        "e_hat1": { "parent": "e_box1/e_lid1", "Transform": {} } } }"#;
    const OTHER: &str = r#"{ "$schema": "ara://schema/scene/1", "id": "sc_other001", "records": {
        "e_box2": { "prefab": "@prefabs/crate.prefab#pf_crate001" } } }"#;

    /// Xorshift: the same sequence every run.
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
        fn pick<T: Clone>(&mut self, items: &[T]) -> T {
            items[self.below(items.len())].clone()
        }
    }

    fn store() -> Store {
        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        ara_components::register(&mut db);
        let docs = [
            ("pf_crate001", "prefabs/crate.prefab", "prefab", CRATE),
            ("pf_stack001", "prefabs/stack.prefab", "prefab", STACK),
            ("sc_level001", "levels/level.scene", "scene", SCENE),
            ("sc_other001", "levels/other.scene", "scene", OTHER),
        ];
        let mut index = AssetIndex::new();
        for (id, path, kind, _) in docs {
            index.insert(id, path, Some(kind.into()));
        }
        let mut ws = Workspace::new(index.clone());
        for (_, path, _, text) in docs {
            ws.insert(path, read(text, path, &db, Some(&index)).doc.expect("doc"));
        }
        Store::new(db, ws)
    }

    fn id(s: &str) -> RecordId {
        RecordId::parse(s).expect("record id")
    }
    fn set(doc: &AssetId, record: RecordId, path: &str, value: Option<Value>) -> Edit {
        Edit::Set {
            doc: doc.clone(),
            id: record,
            path: path.parse::<FieldPath>().expect("path"),
            value,
        }
    }
    fn record(value: Value) -> Record {
        match value {
            Value::Object(map) => map,
            _ => unreachable!("records are objects"),
        }
    }

    #[test]
    fn presentation_recovers_unloaded_documents_without_relaxing_strict_resolution() {
        let mut store = store();
        let mut resolver = SceneResolver::default();
        let before = resolver.presentation(&store, "sc_level001").unwrap();
        let revision = store.revision();
        let (path, document) = store.unload_doc("sc_level001").unwrap();
        assert!(resolver.resolve(&store, "sc_level001").is_err());
        assert!(
            resolver
                .presentation(&store, "sc_level001")
                .unwrap()
                .is_empty()
        );
        assert!(
            resolver
                .presentation(&store, "levels/level.scene")
                .unwrap()
                .is_empty()
        );
        store.load_doc(path, document);
        assert_eq!(
            *resolver.presentation(&store, "sc_level001").unwrap(),
            *before
        );
        assert_eq!(
            store.revision(),
            revision,
            "Display never creates fallback records"
        );

        // A loaded scene's invalid prefab graph is still an actual resolution error.
        // Only the absent active document has an empty presentation state.
        let (path, prefab) = store.unload_doc("pf_crate001").unwrap();
        assert!(resolver.presentation(&store, "sc_level001").is_err());
        store.load_doc(path, prefab);
        assert_eq!(
            *resolver.presentation(&store, "sc_level001").unwrap(),
            *before
        );
    }

    /// One transaction's edits: a scene record's fields, an instance's overrides and
    /// removals, a record created, deleted or reparented (inside an instance too, or
    /// under a missing parent), or the crate prefab itself edited.
    fn edits(rng: &mut Rng, store: &Store, made: &mut usize) -> Vec<Edit> {
        let scene = AssetId::parse("sc_level001").expect("id");
        let prefab = AssetId::parse("pf_crate001").expect("id");
        let Some(document) = store.workspace().doc(scene.as_str()) else {
            return Vec::new();
        };
        let records: Vec<RecordId> = document.records().keys().cloned().collect();
        let instances: Vec<RecordId> = document
            .records()
            .iter()
            .filter(|(_, r)| r.contains_key("prefab"))
            .map(|(id, _)| id.clone())
            .collect();
        if records.is_empty() || instances.is_empty() {
            return Vec::new();
        }
        let any = rng.pick(&records);
        let instance = rng.pick(&instances);
        let parents = [
            None,
            Some("e_lamp1"),
            Some("e_box1"),
            Some("e_box1/e_lid1"),
            Some("e_pile1/e_c002/e_lid1"),
            Some("e_lamp1"),
            Some("e_gone1"),
        ];
        let parent = rng.pick(&parents);
        // A lid inside the instance; now and then an entity its prefab lacks.
        let stack = document.records()[&instance]["prefab"]
            .as_str()
            .is_some_and(|p| p.contains("stack"));
        let inner = match (rng.below(10), stack) {
            (0, _) => "e_nope1",
            (_, true) => "e_c002/e_lid1",
            (_, false) => "e_lid1",
        };
        match rng.below(12) {
            0 | 1 => vec![set(
                &scene,
                any,
                "Transform.position",
                Some(json!([rng.below(9), 0, 0])),
            )],
            2 => vec![set(
                &scene,
                any,
                "name",
                Some(json!(format!("N{}", rng.below(9)))),
            )],
            3 | 4 => {
                let mut overrides = serde_json::Map::new();
                if rng.below(2) == 0 {
                    overrides.insert("Transform.position".into(), json!([rng.below(9), 0, 0]));
                }
                if rng.below(2) == 0 {
                    let scale = json!({"scale": [rng.below(9), 1, 1]});
                    overrides.insert(format!("{inner}/Transform"), scale);
                }
                let overrides = (!overrides.is_empty()).then_some(Value::Object(overrides));
                vec![set(&scene, instance, "overrides", overrides)]
            }
            5 => {
                let removed = (rng.below(2) == 0).then(|| json!([inner]));
                vec![set(&scene, instance, "removed", removed)]
            }
            6 | 7 => {
                *made += 1;
                let mut new = if rng.below(3) == 0 {
                    json!({"prefab": "@prefabs/crate.prefab#pf_crate001"})
                } else {
                    json!({"name": "Made", "Transform": {}})
                };
                if let Some(parent) = parent {
                    new["parent"] = json!(parent);
                }
                vec![Edit::Create {
                    doc: scene,
                    id: id(&format!("e_m{made}")),
                    record: record(new),
                }]
            }
            8 => vec![Edit::Delete {
                doc: scene,
                id: any,
            }],
            9 => vec![Edit::Move {
                doc: scene,
                id: any,
                parent: parent.map(str::to_owned),
                order: None,
            }],
            // The prefab: a field (with a scene edit in the same transaction), a
            // record added or removed.
            _ => match rng.below(3) {
                0 => vec![
                    set(
                        &prefab,
                        id("e_lid1"),
                        "Transform.position",
                        Some(json!([0, rng.below(9), 0])),
                    ),
                    set(&scene, any, "name", Some(json!("Both"))),
                ],
                1 => vec![Edit::Create {
                    doc: prefab,
                    id: id("e_bolt1"),
                    record: record(json!({"parent": "e_crt1", "Transform": {}})),
                }],
                _ => vec![Edit::Delete {
                    doc: prefab,
                    id: id("e_bolt1"),
                }],
            },
        }
    }

    /// After every step of a seeded random sequence of edits, undo and redo, prefab
    /// edits, a prefab unloaded and loaded again, another scene opened and earlier
    /// resolutions held or released, the cache answers exactly as a full resolve:
    /// the same entities, or the same problems.
    #[test]
    fn the_cache_resolves_as_a_full_resolve_after_any_edits() {
        let mut store = store();
        let mut resolver = SceneResolver::default();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut made, mut unloaded) = (0, None);
        let mut held: Vec<Arc<Resolved>> = Vec::new();
        // How often each way of answering came up, and how the answers went.
        let (mut exact, mut in_place, mut copied, mut full) = (0, 0, 0, 0);
        let (mut ok, mut failed, mut committed) = (0, 0, 0);
        for step in 0..1500 {
            match rng.below(20) {
                0 | 1 => {
                    let _ = store.undo_last(&Author::Human);
                }
                2 => {
                    let _ = store.redo_last(&Author::Human);
                }
                // The prefab goes away for a few steps: no transaction, a new generation.
                3 if unloaded.is_none() => unloaded = store.unload_doc("pf_crate001"),
                3..=6 if unloaded.is_some() => {
                    let (path, doc) = unloaded.take().expect("unloaded");
                    store.load_doc(path, doc);
                }
                5 => held.clear(),
                _ => {
                    let edits = edits(&mut rng, &store, &mut made);
                    let commit = store.transact(TxnMeta::by(Author::Human), edits);
                    committed += usize::from(commit.is_ok());
                }
            }
            // Sometimes several transactions pass between resolves.
            if rng.below(3) == 0 {
                continue;
            }
            let doc = if rng.below(12) == 0 {
                "levels/other.scene"
            } else {
                "levels/level.scene"
            };
            let scene = store.workspace().find(doc).cloned().expect("scene");
            let (revision, generation) = (store.revision(), store.workspace().generation());
            let usable: Vec<&Buffer> = resolver
                .buffers
                .iter()
                .filter(|b| b.doc == scene && b.generation == generation)
                .filter(|b| touched(&store, &scene, b.revision, revision).is_some())
                .collect();
            if usable.iter().any(|b| b.revision == revision) {
                exact += 1;
            } else if usable.iter().any(|b| !b.held()) {
                in_place += 1;
            } else if !usable.is_empty() {
                copied += 1;
            } else {
                full += 1;
            }
            let cached = resolver.resolve(&store, doc);
            let whole = super::super::resolve(&store, doc);
            match (&cached, &whole) {
                (Ok(cached), Ok(whole)) => {
                    assert_eq!(**cached, *whole, "step {step}");
                    ok += 1;
                }
                (Err(cached), Err(whole)) => {
                    assert_eq!(cached, whole, "step {step}");
                    failed += 1;
                }
                _ => panic!("step {step}: {cached:?}\nvs\n{whole:?}"),
            }
            if let Ok(resolved) = cached
                && rng.below(2) == 0
            {
                if held.len() == 2 {
                    held.remove(rng.below(2));
                }
                held.push(resolved);
            }
        }
        let counts = [exact, in_place, copied, full, ok, failed, committed];
        assert!(counts.iter().all(|&n| n >= 20), "{counts:?}");
    }
}
