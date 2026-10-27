//! Parse one Hierarchy query once, then retain direct matches and their ancestry.
use ara_doc::prefab::{Resolved, ResolvedEntity};
use std::collections::BTreeSet;

enum Field {
    Any,
    Name,
    Id,
    Type,
}
struct Term {
    field: Field,
    value: String,
    exclude: bool,
}
pub(crate) struct Query {
    terms: Vec<Term>,
}
impl Query {
    pub(in crate::shell) fn new(source: &str) -> Self {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quote = None;
        for ch in source.chars() {
            match ch {
                '\'' | '"' if quote == Some(ch) => quote = None,
                '\'' | '"' if quote.is_none() && (word.is_empty() || word.ends_with(':')) => {
                    quote = Some(ch)
                }
                ch if ch.is_whitespace() && quote.is_none() => {
                    if !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                }
                ch => word.push(ch),
            }
        }
        if !word.is_empty() {
            words.push(word);
        }
        let terms = words
            .into_iter()
            .filter_map(|word| {
                let (exclude, word) = word
                    .strip_prefix('-')
                    .map_or((false, word.as_str()), |word| (true, word));
                let (field, value) = match word.split_once(':') {
                    Some((prefix, value)) => match prefix.to_ascii_lowercase().as_str() {
                        "name" => (Field::Name, value),
                        "id" => (Field::Id, value),
                        "t" | "type" => (Field::Type, value),
                        _ => (Field::Any, word),
                    },
                    None => (Field::Any, word),
                };
                (!value.is_empty()).then(|| Term {
                    field,
                    value: value.to_lowercase(),
                    exclude,
                })
            })
            .collect();
        Self { terms }
    }
    pub(in crate::shell) fn active(&self) -> bool {
        !self.terms.is_empty()
    }
    fn matches(&self, id: &str, entity: &ResolvedEntity) -> bool {
        self.terms.iter().all(|term| {
            let name = entity
                .record
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(id);
            let matches = match term.field {
                Field::Any => {
                    name.to_lowercase().contains(&term.value)
                        || id.to_lowercase().contains(&term.value)
                }
                Field::Name => name.to_lowercase().contains(&term.value),
                Field::Id => id.to_lowercase().contains(&term.value),
                Field::Type => entity.record.keys().any(|kind| {
                    (kind == "prefab" || ara_doc::is_component_key(kind))
                        && kind.to_lowercase().contains(&term.value)
                }),
            };
            matches != term.exclude
        })
    }
}

/// A match stays at its real tree depth, with every live ancestor present.
pub(crate) struct Projection {
    pub(in crate::shell) included: BTreeSet<String>,
}
impl Projection {
    pub(in crate::shell) fn new(resolved: &Resolved, query: &Query) -> Self {
        let direct: Vec<_> = resolved
            .iter()
            .filter(|(id, entity)| query.matches(id, entity))
            .map(|(id, _)| id)
            .collect();
        let mut included = BTreeSet::new();
        for id in &direct {
            let mut next = Some(id.as_str());
            while let Some(id) = next {
                if !included.insert(id.to_owned()) {
                    break;
                }
                next = resolved
                    .get(id)
                    .and_then(|entity| entity.parent.as_deref())
                    .filter(|parent| resolved.contains_key(*parent));
            }
        }
        Self { included }
    }
}
