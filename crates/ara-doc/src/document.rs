//! The in-memory document: a flat map of records keyed by ID.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{AssetId, OrderKey, RecordId};

/// A record: a JSON object of built-in fields (lowercase keys) and, for entities,
/// components (PascalCase keys).
pub type Record = Map<String, Value>;

/// Built-in record field names.
pub mod field {
    /// Display name.
    pub const NAME: &str = "name";
    /// Parent entity address.
    pub const PARENT: &str = "parent";
    /// Fractional-index sibling order.
    pub const ORDER: &str = "order";
    /// Prefab this record instantiates.
    pub const PREFAB: &str = "prefab";
    /// Prefab overrides, keyed by `[<id path>/]<field path>`.
    pub const OVERRIDES: &str = "overrides";
    /// Prefab entities and components removed from this instance.
    pub const REMOVED: &str = "removed";
}

/// A scene, prefab or other authoring document.
///
/// Read with [`crate::read`], write with [`crate::write`]. Mutations go through ops
/// ([`Document::apply`]), which ara-ops wraps in transactions; nothing else writes
/// records.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Document kind name (`scene`).
    pub kind: String,
    /// Project-unique asset ID.
    pub id: AssetId,
    pub(crate) records: BTreeMap<RecordId, Record>,
    /// Unresolved merge conflicts (`$conflicts`), kept verbatim until resolved.
    pub conflicts: Vec<Value>,
}

impl Document {
    /// An empty document.
    pub fn new(kind: impl Into<String>, id: AssetId) -> Self {
        Document {
            kind: kind.into(),
            id,
            records: BTreeMap::new(),
            conflicts: Vec::new(),
        }
    }

    /// Records sorted by ID.
    pub fn records(&self) -> &BTreeMap<RecordId, Record> {
        &self.records
    }

    /// One record.
    pub fn record(&self, id: &str) -> Option<&Record> {
        self.records.get(id)
    }

    /// The children of `parent` (an entity address, or `None` for roots), in sibling
    /// order: by `order`, then by ID. Records without `order` sort first.
    pub fn children(&self, parent: Option<&str>) -> Vec<&RecordId> {
        let mut kids: Vec<(&str, &RecordId)> = self
            .records
            .iter()
            .filter(|(_, r)| parent_of(r) == parent)
            .map(|(id, r)| (order_of(r).unwrap_or(""), id))
            .collect();
        kids.sort();
        kids.into_iter().map(|(_, id)| id).collect()
    }

    /// IDs of `id` and every record below it (through `parent` links, including
    /// records parented inside its prefab instance), children before parents.
    pub fn subtree(&self, id: &RecordId) -> Vec<RecordId> {
        let mut out = Vec::new();
        self.collect_subtree(id, &mut out);
        out
    }

    fn collect_subtree(&self, id: &RecordId, out: &mut Vec<RecordId>) {
        let kids: Vec<RecordId> = self
            .records
            .iter()
            .filter(|(_, r)| parent_of(r).is_some_and(|p| p.split('/').next() == Some(id.as_str())))
            .map(|(k, _)| k.clone())
            .collect();
        for kid in &kids {
            self.collect_subtree(kid, out);
        }
        out.push(id.clone());
    }

    /// Inserts a record without validation. Readers and merges use this; editing goes
    /// through ops.
    pub fn insert_unchecked(&mut self, id: RecordId, record: Record) {
        self.records.insert(id, record);
    }
}

/// The record's `parent` address.
pub fn parent_of(r: &Record) -> Option<&str> {
    r.get(field::PARENT).and_then(Value::as_str)
}

/// The record's `order` key text.
pub fn order_of(r: &Record) -> Option<&str> {
    r.get(field::ORDER).and_then(Value::as_str)
}

/// The record's parsed `order` key.
pub fn order_key(r: &Record) -> Option<OrderKey> {
    order_of(r).and_then(|s| OrderKey::parse(s).ok())
}

/// The record's `prefab` reference, if it is a prefab instance.
pub fn prefab_of(r: &Record) -> Option<&str> {
    r.get(field::PREFAB).and_then(Value::as_str)
}

/// True for component keys (PascalCase).
pub fn is_component_key(key: &str) -> bool {
    key.starts_with(|c: char| c.is_ascii_uppercase())
}
