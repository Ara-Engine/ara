//! The project's import settings as the Scene reads them, parsed once per change.
use std::collections::BTreeMap;
use std::sync::Arc;

use ara_doc::Op;
use ara_ops::Store;
use serde_json::Value;

/// Import settings by source path: each `*.import.json` document's `i_settings`.
pub type Imports = BTreeMap<String, ara_asset::ImportSettings>;

/// Every loaded import document's settings, by its source asset's path.
pub fn parse(store: &Store) -> Imports {
    let ws = store.workspace();
    ws.docs()
        .filter(|(_, d)| d.kind == "import")
        .filter_map(|(_, d)| {
            let path = ws.index().path_of(d.id.as_str())?;
            let settings =
                serde_json::from_value(Value::Object(d.record("i_settings")?.clone())).ok()?;
            Some((path.to_owned(), settings))
        })
        .collect()
}

/// [`parse`], kept across revisions that touch no import document: every commit's
/// view used to parse them all (~0.3 ms on Outpost). Parses again after an op on an
/// import document, a document's lifecycle, a document loaded, replaced or unloaded
/// (`Workspace::generation`) or a gap in the history. `Shell::document_view` keeps
/// one; views of the same settings share them, so `DocumentView::patch_to` compares
/// them by pointer.
#[derive(Default)]
pub struct ImportsCache {
    /// The workspace generation and store revision `settings` reflects.
    checked: Option<(u64, u64)>,
    settings: Arc<Imports>,
}

impl ImportsCache {
    /// The import settings at the store's current revision.
    pub fn get(&mut self, store: &Store) -> Arc<Imports> {
        let now = (store.workspace().generation(), store.revision());
        let current = self.checked.is_some_and(|(generation, revision)| {
            generation == now.0 && untouched(store, revision)
        });
        if !current {
            self.settings = Arc::new(parse(store));
        }
        self.checked = Some(now);
        Arc::clone(&self.settings)
    }
}

/// Whether every transaction after `from` is known and left import documents alone.
fn untouched(store: &Store, from: u64) -> bool {
    let to = store.revision();
    if from > to {
        return false;
    }
    let since = store.since(from);
    let ws = store.workspace();
    since.len() as u64 == to - from
        && since.iter().flat_map(|t| &t.ops).all(|op| {
            !matches!(op, Op::Document { .. })
                && ws
                    .doc(op.doc().as_str())
                    .is_some_and(|d| d.kind != "import")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_doc::{AssetIndex, Edit, RecordId, Workspace, read};
    use ara_ops::{Author, TxnMeta};
    use ara_schema::{FieldPath, SchemaDb};
    use serde_json::json;

    const PAINT: &str = r#"{ "$schema": "ara://schema/import/1", "id": "as_paint001",
        "records": { "i_settings": { "importer": "texture", "version": "1", "settings": {} } } }"#;
    const ROCK: &str = r#"{ "$schema": "ara://schema/import/1", "id": "as_rock0001",
        "records": { "i_settings": { "importer": "model", "version": "1", "settings": {} } } }"#;
    const SCENE: &str = r#"{ "$schema": "ara://schema/scene/1", "id": "sc_level001",
        "records": { "e_box1": { "name": "Box", "Transform": {} } } }"#;

    fn same(a: &Imports, b: &Imports) -> bool {
        serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
    }
    fn set(doc: &str, id: &str, path: &str, value: Value) -> Edit {
        Edit::Set {
            doc: ara_doc::AssetId::parse(doc).expect("id"),
            id: RecordId::parse(id).expect("id"),
            path: path.parse::<FieldPath>().expect("path"),
            value: Some(value),
        }
    }

    /// The settings are reused across scene edits and parsed again, matching a fresh
    /// parse, after an edit to an import document and after a document is loaded.
    #[test]
    fn settings_are_parsed_again_only_when_an_import_document_changes() {
        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        ara_components::register(&mut db);
        ara_asset::register_schema(&mut db);
        let mut index = AssetIndex::new();
        index.insert("sc_level001", "levels/level.scene", Some("scene".into()));
        index.insert("as_paint001", "paint.png", None);
        index.insert("as_rock0001", "rock.glb", None);
        let mut ws = Workspace::new(index.clone());
        for (path, asset, text) in [
            ("levels/level.scene", "levels/level.scene", SCENE),
            ("paint.png.import.json", "paint.png", PAINT),
        ] {
            let doc = read(text, path, &db, Some(&index)).doc.expect("doc");
            ws.insert_with_asset_path(path, doc, asset);
        }
        let mut store = Store::new(db, ws);
        let mut cache = ImportsCache::default();
        let first = cache.get(&store);
        assert_eq!(first.len(), 1);
        assert!(Arc::ptr_eq(&first, &cache.get(&store)));
        // A scene edit leaves them alone.
        let by = || TxnMeta::by(Author::Human);
        store
            .transact(by(), [set("sc_level001", "e_box1", "name", json!("Crate"))])
            .expect("scene edit");
        assert!(Arc::ptr_eq(&first, &cache.get(&store)));
        // An import document's edit parses them again.
        store
            .transact(
                by(),
                [set("as_paint001", "i_settings", "version", json!("2"))],
            )
            .expect("import edit");
        let edited = cache.get(&store);
        assert!(!Arc::ptr_eq(&first, &edited));
        assert_eq!(edited["paint.png"].version, "2");
        assert!(same(&edited, &parse(&store)));
        // So does a document loaded outside a transaction.
        let rock = read(ROCK, "rock.glb.import.json", store.schema(), Some(&index))
            .doc
            .expect("doc");
        store.load_doc_with_asset_path("rock.glb.import.json", rock, "rock.glb");
        let loaded = cache.get(&store);
        assert_eq!(loaded.len(), 2);
        assert!(same(&loaded, &parse(&store)));
    }
}
