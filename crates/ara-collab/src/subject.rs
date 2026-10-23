//! What can be claimed: a document, an entity, a component or field of an entity,
//! or a project asset path (C# modules, media). Subjects decide which ops a claim
//! covers.

use std::fmt;

use ara_doc::{AssetId, Op, RecordId};
use ara_schema::FieldPath;
use serde::{Deserialize, Serialize};

/// A claimable subject.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Subject {
    /// A whole document.
    Doc {
        /// Document asset ID.
        doc: AssetId,
    },
    /// One entity record (an address inside a prefab instance claims the instance).
    Entity {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        entity: RecordId,
    },
    /// A component or field of an entity (`Light`, `Transform.position`).
    Field {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        entity: RecordId,
        /// Field path inside the record.
        path: FieldPath,
    },
    /// A project file that is not a loaded document (`@scripts/Gate.cs`). Advisory
    /// until the asset and code pipelines check claims.
    Asset {
        /// Project path, `@`-prefixed.
        path: String,
    },
}

impl Subject {
    /// The document, for document subjects.
    pub fn doc(&self) -> Option<&AssetId> {
        match self {
            Subject::Doc { doc } | Subject::Entity { doc, .. } | Subject::Field { doc, .. } => {
                Some(doc)
            }
            Subject::Asset { .. } => None,
        }
    }

    /// The record and path inside the document (`None` path: the whole record; `None`
    /// record: the whole document).
    fn scope(&self) -> Option<(Option<&RecordId>, Option<&FieldPath>)> {
        match self {
            Subject::Doc { .. } => Some((None, None)),
            Subject::Entity { entity, .. } => Some((Some(entity), None)),
            Subject::Field { entity, path, .. } => Some((Some(entity), Some(path))),
            Subject::Asset { .. } => None,
        }
    }

    /// True if edits to one interfere with the other.
    pub fn overlaps(&self, other: &Subject) -> bool {
        if let (Subject::Asset { path: a }, Subject::Asset { path: b }) = (self, other) {
            return a == b;
        }
        if self.doc().is_none() || self.doc() != other.doc() {
            return false;
        }
        match (self.scope(), other.scope()) {
            (Some(a), Some(b)) => scopes_overlap(a, b),
            _ => false,
        }
    }

    /// True if `op` changes something this subject covers.
    pub fn touched_by(&self, op: &Op) -> bool {
        let Some((record, path)) = self.scope() else {
            return false;
        };
        if self.doc() != Some(op.doc()) {
            return false;
        }
        op.touches()
            .iter()
            .any(|t| scopes_overlap((record, path), (Some(op.id()), t.as_ref())))
    }
}

fn scopes_overlap(
    (ra, pa): (Option<&RecordId>, Option<&FieldPath>),
    (rb, pb): (Option<&RecordId>, Option<&FieldPath>),
) -> bool {
    match (ra, rb) {
        (Some(a), Some(b)) if a != b => false,
        (Some(_), Some(_)) => match (pa, pb) {
            (Some(a), Some(b)) => a.overlaps(b),
            _ => true,
        },
        _ => true,
    }
}

impl fmt::Display for Subject {
    /// `sc_x`, `sc_x#e_1`, `sc_x#e_1.Light`, `@scripts/Gate.cs`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::Doc { doc } => write!(f, "{doc}"),
            Subject::Entity { doc, entity } => write!(f, "{doc}#{entity}"),
            Subject::Field { doc, entity, path } => write!(f, "{doc}#{entity}.{path}"),
            Subject::Asset { path } => f.write_str(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> AssetId {
        AssetId::parse("sc_test0001").expect("id")
    }

    fn entity(id: &str) -> Subject {
        Subject::Entity {
            doc: doc(),
            entity: RecordId::parse(id).expect("id"),
        }
    }

    fn field(id: &str, path: &str) -> Subject {
        Subject::Field {
            doc: doc(),
            entity: RecordId::parse(id).expect("id"),
            path: path.parse().expect("path"),
        }
    }

    fn set(id: &str, path: &str) -> Op {
        Op::Set {
            doc: doc(),
            id: RecordId::parse(id).expect("id"),
            path: path.parse().expect("path"),
            value: None,
            prev: None,
        }
    }

    #[test]
    fn subjects_cover_what_they_name() {
        let d = Subject::Doc { doc: doc() };
        assert!(d.touched_by(&set("e_a", "name")));
        assert!(entity("e_a").touched_by(&set("e_a", "Transform.position")));
        assert!(!entity("e_a").touched_by(&set("e_b", "name")));
        assert!(field("e_a", "Transform").touched_by(&set("e_a", "Transform.position.x")));
        assert!(!field("e_a", "Transform").touched_by(&set("e_a", "name")));
        assert!(field("e_a", "Transform.position").overlaps(&entity("e_a")));
        assert!(!field("e_a", "Transform.position").overlaps(&field("e_a", "Transform.scale")));
        assert!(d.overlaps(&field("e_z", "name")));
        let asset = Subject::Asset {
            path: "@scripts/Gate.cs".into(),
        };
        assert!(asset.overlaps(&asset.clone()) && !asset.overlaps(&d));
        assert_eq!(
            field("e_a", "Light.intensity").to_string(),
            "sc_test0001#e_a.Light.intensity"
        );
    }
}
