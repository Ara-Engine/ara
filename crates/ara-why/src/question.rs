//! Free-text questions, split into what explainers match on: entity selectors,
//! other subjects, diagnostic codes and topic words.

use ara_diag::{Code, Subject, SubjectKind, catalog};

/// Words that carry no topic ("why is the … not …").
const STOP: &[&str] = &[
    "why", "is", "isn't", "isnt", "are", "aren't", "was", "wasn't", "the", "a", "an", "not", "no",
    "does", "doesn't", "doesnt", "did", "didn't", "didnt", "do", "don't", "it", "its", "my",
    "this", "that", "these", "those", "in", "on", "at", "of", "to", "for", "from", "with", "so",
    "still", "anymore", "any", "more", "can't", "cant", "cannot", "i", "we", "me", "be", "being",
    "been", "and", "or", "what", "when", "how", "where", "has", "have", "had", "entity", "object",
    "thing", "about", "there", "here", "now", "all", "game", "level", "scene", "world", "anymore",
    "after", "since", "someone", "suddenly", "up", "out", "any",
];

/// A parsed `ara why` question.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Question {
    /// The question as asked.
    pub text: String,
    /// Entity selectors: `name:Crate`, `e_8b21`, `e_4f9a/e_m4tz`, `@levels/a.scene#e_8b21`.
    pub entities: Vec<String>,
    /// Other subjects: assets, documents, systems, passes, modules.
    pub subjects: Vec<Subject>,
    /// Diagnostic codes (`ARA1302`, `ARA-EXP-AND-007`, `UNRESOLVED_REF`).
    pub codes: Vec<Code>,
    /// Lower-case words that are neither stop words nor subjects.
    pub words: Vec<String>,
    /// Capitalized words that may be entity names (`Crate` in "why is Crate hidden").
    pub names: Vec<String>,
    /// `doc:` / `in:` document hint (`doc:outpost`).
    pub doc: Option<String>,
}

/// Splits on whitespace, keeping quoted spans (`name:"Big Crate"`) together and
/// stripping outer punctuation.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') if cur.ends_with(':') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter()
        .map(|t| {
            t.trim_matches(|c: char| matches!(c, '?' | ',' | '!' | '`' | '"' | '(' | ')' | ';'))
                .trim_end_matches('.')
                .to_owned()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

fn is_entity_id(t: &str) -> bool {
    t.split('/').all(|p| {
        p.strip_prefix("e_").is_some_and(|r| {
            !r.is_empty() && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    })
}

impl Question {
    /// Parses free text.
    pub fn parse(text: &str) -> Question {
        let mut q = Question {
            text: text.trim().to_owned(),
            ..Question::default()
        };
        for t in tokens(text) {
            let lower = t.to_lowercase();
            if let Some(doc) = lower
                .strip_prefix("doc:")
                .or_else(|| lower.strip_prefix("in:"))
                .map(|_| t.split_once(':').map_or("", |(_, d)| d))
            {
                q.doc = Some(doc.to_owned());
                continue;
            }
            if lower.starts_with("name:") || is_entity_id(&t) {
                q.entities.push(t);
                continue;
            }
            if let Some(code) = code_of(&t) {
                if !q.codes.contains(&code) {
                    q.codes.push(code);
                }
                continue;
            }
            if (t.contains(':') || t.starts_with('@'))
                && let Some(s) = Subject::parse(&t)
            {
                match s.kind {
                    SubjectKind::Entity => q.entities.push(s.value),
                    _ => q.subjects.push(s),
                }
                continue;
            }
            if STOP.contains(&lower.as_str()) {
                continue;
            }
            if t.chars().next().is_some_and(char::is_uppercase)
                && t.chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
            {
                q.names.push(t.clone());
            }
            q.words.push(lower);
        }
        q
    }

    /// True if any word starts with one of `stems` (`visib` matches `visible`).
    pub fn about(&self, stems: &[&str]) -> bool {
        self.words
            .iter()
            .any(|w| stems.iter().any(|s| w.starts_with(s)))
    }

    /// Words not starting with any of `stems`.
    pub fn other_words(&self, stems: &[&str]) -> Vec<&str> {
        self.words
            .iter()
            .filter(|w| !stems.iter().any(|s| w.starts_with(s)))
            .map(String::as_str)
            .collect()
    }

    /// Words that may name an entity, best first: capitalized words, then adjacent
    /// pairs joined in title case (`gate crate` → `GateCrate`), then single words in
    /// title case (`workbench` → `Workbench`). Topic words (`stems`) are left out.
    pub fn name_candidates(&self, stems: &[&str]) -> Vec<String> {
        let title = |w: &str| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        };
        let words = self.other_words(stems);
        let mut out: Vec<String> = self.names.clone();
        out.extend(
            words
                .windows(2)
                .map(|w| format!("{}{}", title(w[0]), title(w[1]))),
        );
        out.extend(words.iter().map(|w| title(w)));
        let mut seen = std::collections::BTreeSet::new();
        out.retain(|c| seen.insert(c.clone()));
        out
    }

    /// The first subject of `kind`.
    pub fn subject(&self, kind: SubjectKind) -> Option<&Subject> {
        self.subjects.iter().find(|s| s.kind == kind)
    }
}

/// A token naming a diagnostic code: `ARA1302`, a named code, or a constant name in
/// capitals with an underscore (`UNRESOLVED_REF`).
fn code_of(t: &str) -> Option<Code> {
    let upper = t.to_uppercase();
    if upper.starts_with("ARA") {
        return catalog::lookup(t)
            .map(|d| d.code)
            .or_else(|| Code::parse(t));
    }
    let constant = t.contains('_')
        && t.chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c == ':' || c.is_ascii_digit());
    constant
        .then(|| catalog::lookup(t).map(|d| d.code))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_selectors_subjects_codes_and_words() {
        let q = Question::parse("why is `name:Crate` not visible in doc:outpost?");
        assert_eq!(q.entities, ["name:Crate"]);
        assert_eq!(q.doc.as_deref(), Some("outpost"));
        assert_eq!(q.words, ["visible"]);
        assert!(q.about(&["visib"]));

        let q = Question::parse("why ara1302 on @meshes/crate.glb and e_4f9a/e_m4tz");
        assert_eq!(q.codes, [Code(1302)]);
        assert_eq!(q.subjects, [Subject::asset("@meshes/crate.glb")]);
        assert_eq!(q.entities, ["e_4f9a/e_m4tz"]);

        let q = Question::parse(r#"why is name:"Big Crate" hidden"#);
        assert_eq!(q.entities, ["name:Big Crate"]);

        let q = Question::parse("why did module:Gameplay reload fall back to L3");
        assert_eq!(q.subjects, [Subject::module("Gameplay")]);
        assert_eq!(q.words, ["reload", "fall", "back", "l3"]);

        let q = Question::parse("why is Crate invisible");
        assert_eq!(q.names, ["Crate"]);
        let q = Question::parse("why is the gate crate not rendering in game");
        assert_eq!(
            q.name_candidates(&["render"]),
            ["GateCrate", "Gate", "Crate"]
        );
    }
}
