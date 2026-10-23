use crate::{Result, TextIndex, invalid};
use ara_schema::{FieldType, ValueType};
use serde::{Deserialize, Deserializer, Serialize};

/// A world-wide entity reference, serialized as `@path#e_id`. The entity ID wins
/// when a cell move makes the path hint stale. Reflected as a string so the kernel's
/// asset repair cannot replace the entity ID with the scene document's asset ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct WorldEntityRef(String);

impl WorldEntityRef {
    /// Validates a complete portable path and stable entity record ID.
    pub fn parse(reference: impl Into<String>) -> Result<Self> {
        let reference = reference.into();
        let (path, id) = crate::model::document_ref(&reference)?;
        if path.is_empty() || ara_doc::RecordId::parse(id).map_err(invalid)?.prefix() != "e" {
            return Err(invalid("world references require an entity ID"));
        }
        Ok(Self(reference))
    }

    /// Canonical string representation, including the readable path hint.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Stable entity identity used for lookup across loaded and unloaded cells.
    pub fn identity(&self) -> &str {
        self.0
            .rsplit_once('#')
            .expect("validated world references contain a stable ID")
            .1
    }

    /// Resolves by identity and refreshes the path from the immutable text index.
    pub fn resolve(&self, index: &TextIndex) -> Result<Self> {
        let entity = index
            .entity(self.identity())
            .ok_or_else(|| invalid("unresolved world entity reference"))?;
        Self::parse(entity.reference.to_owned())
    }
}

impl FieldType for WorldEntityRef {
    fn value_type() -> ValueType {
        ValueType::String
    }
}
impl<'de> Deserialize<'de> for WorldEntityRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
