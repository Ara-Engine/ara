//! What a diagnostic is about: entities, assets, documents, systems, render passes
//! and C# modules, as compact `kind:value` strings (`entity:@levels/a.scene#e_8b21`).
//!
//! Subjects are how the bus is queried ("everything about the crate") and how
//! `ara why` follows causes from one thing to another. References keep the
//! `@path#id` form; either half matches (the ID is authoritative, the path repairs).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The kind of thing a [`Subject`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubjectKind {
    /// A scene entity: `e_8b21`, an address `e_4f9a/e_m4tz` or `@levels/a.scene#e_8b21`.
    Entity,
    /// An asset: `@meshes/crate.glb#ms_crate001` (either half may be left out).
    Asset,
    /// A document: `@levels/arena.scene#sc_arena01` or its path.
    Document,
    /// An ECS or engine system (`physics.step`, `Gameplay.DoorSystem`).
    System,
    /// A render pass (`gbuffer`, `shadows`).
    Pass,
    /// A C# module (`Gameplay`).
    Module,
}

impl SubjectKind {
    /// Every kind, in display order.
    pub const ALL: [SubjectKind; 6] = [
        SubjectKind::Entity,
        SubjectKind::Asset,
        SubjectKind::Document,
        SubjectKind::System,
        SubjectKind::Pass,
        SubjectKind::Module,
    ];

    /// The prefix in the string form (`entity`).
    pub fn as_str(self) -> &'static str {
        match self {
            SubjectKind::Entity => "entity",
            SubjectKind::Asset => "asset",
            SubjectKind::Document => "document",
            SubjectKind::System => "system",
            SubjectKind::Pass => "pass",
            SubjectKind::Module => "module",
        }
    }
}

/// One thing a diagnostic is about. The string form is `kind:value`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subject {
    /// What kind of thing.
    pub kind: SubjectKind,
    /// Its reference, ID, address or name.
    pub value: String,
}

impl Subject {
    /// A subject of `kind`.
    pub fn new(kind: SubjectKind, value: impl Into<String>) -> Self {
        Subject {
            kind,
            value: value.into(),
        }
    }

    /// An entity (`e_8b21`, `e_4f9a/e_m4tz`, `@levels/a.scene#e_8b21`).
    pub fn entity(value: impl Into<String>) -> Self {
        Self::new(SubjectKind::Entity, value)
    }

    /// An asset (`@meshes/crate.glb#ms_crate001`).
    pub fn asset(value: impl Into<String>) -> Self {
        Self::new(SubjectKind::Asset, value)
    }

    /// A document (`@levels/arena.scene#sc_arena01` or `levels/arena.scene`).
    pub fn document(value: impl Into<String>) -> Self {
        Self::new(SubjectKind::Document, value)
    }

    /// A system (`physics.step`).
    pub fn system(value: impl Into<String>) -> Self {
        Self::new(SubjectKind::System, value)
    }

    /// A render pass (`gbuffer`).
    pub fn pass(value: impl Into<String>) -> Self {
        Self::new(SubjectKind::Pass, value)
    }

    /// A C# module (`Gameplay`).
    pub fn module(value: impl Into<String>) -> Self {
        Self::new(SubjectKind::Module, value)
    }

    /// Parses `kind:value`, or infers the kind of a bare value: `@path#e_…` and
    /// `e_…` are entities, other `@` references assets (documents when the path
    /// has a document extension).
    pub fn parse(text: &str) -> Option<Subject> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if let Some((prefix, value)) = text.split_once(':')
            && let Some(kind) = SubjectKind::ALL.iter().find(|k| k.as_str() == prefix)
        {
            return (!value.is_empty()).then(|| Subject::new(*kind, value));
        }
        let (path, id) = split_ref(text);
        let kind = match (path, id) {
            (_, Some(id)) if is_entity_id(id) => SubjectKind::Entity,
            (None, None) if is_entity_id(text) => SubjectKind::Entity,
            (Some(p), _) if is_document_path(p) => SubjectKind::Document,
            _ if text.starts_with('@') => SubjectKind::Asset,
            _ => return None,
        };
        Some(Subject::new(kind, text))
    }

    /// The `@path#id` halves of the value (both `None` for plain names).
    pub fn parts(&self) -> (Option<&str>, Option<&str>) {
        split_ref(&self.value)
    }

    /// True if `self` (a query) matches `other` (a recorded subject): same kind and
    /// the same thing. Reference halves match independently (the same ID, or the
    /// same path); an entity matches the entities inside it (`e_4f9a` matches
    /// `e_4f9a/e_m4tz`); names compare case-insensitively.
    pub fn matches(&self, other: &Subject) -> bool {
        if self.kind != other.kind {
            return false;
        }
        if self.value == other.value {
            return true;
        }
        let (qp, qi) = self.parts();
        let (op, oi) = other.parts();
        let ids = |a: &str, b: &str| {
            a == b || (self.kind == SubjectKind::Entity && b.starts_with(a) && b[a.len()..].starts_with('/'))
        };
        match (qi, oi) {
            (Some(a), Some(b)) if ids(a, b) => return true,
            (Some(_), Some(_)) => {}
            _ => {}
        }
        if let (Some(a), Some(b)) = (qp, op)
            && a == b
            && (qi.is_none() || oi.is_none())
        {
            return true;
        }
        if qp.is_none() && qi.is_none() {
            return self.value.eq_ignore_ascii_case(&other.value);
        }
        false
    }
}

/// Splits `@path#id` (or a bare `id`, or `path`) into its halves. Values without
/// `@` or `#` are plain IDs when they look like one (`e_8b21`), else names.
fn split_ref(text: &str) -> (Option<&str>, Option<&str>) {
    let Some(rest) = text.strip_prefix('@') else {
        return if is_entity_id(text) || looks_like_id(text) {
            (None, Some(text))
        } else if text.contains('/') && text.contains('.') {
            (Some(text), None)
        } else {
            (None, None)
        };
    };
    match rest.split_once('#') {
        Some((path, id)) => ((!path.is_empty()).then_some(path), (!id.is_empty()).then_some(id)),
        None => (Some(rest), None),
    }
}

/// `e_8b21`, or an address `e_4f9a/e_m4tz`.
fn is_entity_id(text: &str) -> bool {
    text.split('/').all(|part| {
        part.strip_prefix("e_")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    })
}

/// A short asset ID (`ms_crate001`): a lowercase prefix, `_`, then alphanumerics.
fn looks_like_id(text: &str) -> bool {
    text.split_once('_').is_some_and(|(prefix, rest)| {
        (1..=3).contains(&prefix.len())
            && prefix.chars().all(|c| c.is_ascii_lowercase())
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

fn is_document_path(path: &str) -> bool {
    [".scene", ".prefab", ".world", ".material", ".ui"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.value)
    }
}

impl FromStr for Subject {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Subject::parse(s).ok_or_else(|| {
            format!(
                "`{s}` is not a subject; write `entity:e_8b21`, `asset:@meshes/crate.glb`, `system:physics.step`, `pass:gbuffer` or `module:Gameplay`"
            )
        })
    }
}

impl Serialize for Subject {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Subject {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Subject {
        text.parse().expect("subject")
    }

    #[test]
    fn parses_prefixed_and_bare_forms() {
        assert_eq!(s("system:physics.step"), Subject::system("physics.step"));
        assert_eq!(s("e_8b21"), Subject::entity("e_8b21"));
        assert_eq!(s("e_4f9a/e_m4tz").kind, SubjectKind::Entity);
        assert_eq!(s("@levels/a.scene#e_8b21").kind, SubjectKind::Entity);
        assert_eq!(s("@levels/a.scene").kind, SubjectKind::Document);
        assert_eq!(s("@meshes/crate.glb#ms_crate001").kind, SubjectKind::Asset);
        assert!(Subject::parse("Crate").is_none());
        assert_eq!(s("entity:@levels/a.scene#e_8b21").to_string(), "entity:@levels/a.scene#e_8b21");
    }

    #[test]
    fn reference_halves_match_independently() {
        let recorded = s("asset:@meshes/crate.glb#ms_crate001");
        assert!(s("asset:@meshes/crate.glb").matches(&recorded));
        assert!(s("asset:ms_crate001").matches(&recorded));
        assert!(s("asset:@meshes/old.glb#ms_crate001").matches(&recorded));
        assert!(!s("asset:@meshes/lid.glb#ms_lid0001").matches(&recorded));
        assert!(!s("entity:e_crate001").matches(&recorded));
    }

    #[test]
    fn entities_match_their_insides_and_names_ignore_case() {
        assert!(s("e_4f9a").matches(&s("entity:@levels/a.scene#e_4f9a/e_m4tz")));
        assert!(!s("e_4f9").matches(&s("e_4f9a")));
        assert!(s("module:gameplay").matches(&s("module:Gameplay")));
    }
}
