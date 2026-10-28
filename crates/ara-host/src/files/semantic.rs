//! Compare persistence snapshots with the document writer's schema/default rules.
use ara_doc::Document;
use ara_schema::SchemaDb;

/// Normalize a frozen merge input; live authoring state still changes only by ops.
pub(super) fn canonical(doc: &Document, db: &SchemaDb) -> Document {
    let mut normalized = Document::new(doc.kind.clone(), doc.id.clone());
    normalized.conflicts = doc.conflicts.clone();
    let kind = db.kind(&doc.kind);
    for (id, record) in doc.records() {
        let record_type = kind.and_then(|kind| db.record_type(kind, id.prefix()));
        normalized.insert_unchecked(
            id.clone(),
            ara_doc::canonical_record(db, record_type, record),
        );
    }
    normalized
}

/// Explicit defaults and their canonically omitted form describe the same state.
pub(crate) fn equivalent(left: &Document, right: &Document, db: &SchemaDb) -> bool {
    left == right || ara_json::json_eq(&ara_doc::to_value(left, db), &ara_doc::to_value(right, db))
}
