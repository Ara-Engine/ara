//! Structured diagnostics shared by every Ara subsystem, and the bus that carries
//! them to the project host.
//!
//! A [`Diagnostic`] has a stable [`Code`] (`ARA1101`), a severity, a message, a
//! structured [`Location`] (file, record, field path, ...), the [`Subject`]s it is
//! about (entities, assets, systems, passes, modules) and optional
//! machine-applicable [`Fix`]es. The CLI prints them as text or JSON; agents read the
//! JSON and apply fixes directly.
//!
//! - [`bus`]: any thread emits diagnostics (lock-free, sampled); the host drains
//!   them into a [`store::Store`] it queries, streams and summarizes ([`summary`]).
//! - [`codes`]: kernel and bus code constants; [`catalog`]: every code's docs
//!   (title, explanation, causes, fixes), including other crates' codes.

pub mod bus;
pub mod catalog;
pub mod codes;
mod render;
pub mod store;
mod subject;
pub mod summary;

pub use subject::{Subject, SubjectKind};

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// A stable diagnostic code, printed as `ARA` plus four digits (`ARA1101`).
///
/// Codes never change meaning once released. `ARA0001`–`ARA0999` belong to C#; the
/// kernel and bus ranges are listed in [`codes`]. *Named* codes (`ARA-EXP-AND-007`,
/// `ARA_GPU_STALE_HANDLE`) keep the name they were born with: their number is an
/// internal slot and they print and parse by name ([`catalog::label`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Code(pub u16);

impl Code {
    /// Parses `ARA1302` (any case) or a named code's name.
    pub fn parse(text: &str) -> Option<Code> {
        let t = text.trim();
        let numeric = t
            .get(..3)
            .filter(|p| p.eq_ignore_ascii_case("ara"))
            .and_then(|_| t.get(3..))
            .filter(|n| n.len() == 4)
            .and_then(|n| n.parse().ok());
        match numeric {
            Some(n) => Some(Code(n)),
            None => catalog::lookup(t).filter(|d| d.label.is_some()).map(|d| d.code),
        }
    }

    /// Its documentation, if catalogued.
    pub fn doc(self) -> Option<&'static catalog::CodeDoc> {
        catalog::get(self)
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match catalog::label(*self) {
            Some(label) => f.write_str(label),
            None => write!(f, "ARA{:04}", self.0),
        }
    }
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Code {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Code::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid diagnostic code `{text}`")))
    }
}

/// How serious a diagnostic is. Only errors fail `ara check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The input is invalid; the operation was rejected or the file cannot be used.
    Error,
    /// The input works but should change (stale reference path, deprecated field).
    Warning,
    /// Something was normalized automatically (an ID filled in, a type upgraded).
    Info,
}

/// Where a diagnostic points. Every part is optional; set what you know.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    /// Project-relative file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Asset ID of the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Record ID inside the document (`e_8b21`), or an entity address (`e_4f9a/m4tz`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// Field path inside the record or value (`Transform.position`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 1-based line, when known (syntax errors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// 1-based column, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// Index of the command call inside a batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<usize>,
}

impl Location {
    /// True when no part of the location is set.
    pub fn is_empty(&self) -> bool {
        *self == Location::default()
    }
}

/// A machine-applicable fix: a title for humans plus an action for tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    /// One-line description shown to humans ("Rename `positon` to `position`").
    pub title: String,
    /// What to do.
    #[serde(flatten)]
    pub action: FixAction,
}

/// The edit a [`Fix`] performs. Record and path are relative to the diagnostic's
/// document; a missing record means the document root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum FixAction {
    /// Set the value at `path`.
    Set {
        /// Target record, or the document root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record: Option<String>,
        /// Field path to write.
        path: String,
        /// New value.
        value: Value,
    },
    /// Remove the value at `path` (it falls back to its default).
    Remove {
        /// Target record, or the document root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record: Option<String>,
        /// Field path to remove.
        path: String,
    },
    /// Rename the last key of `path` to `to`, keeping its value.
    Rename {
        /// Target record, or the document root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record: Option<String>,
        /// Field path whose last key is renamed.
        path: String,
        /// The new key.
        to: String,
    },
    /// Run a registered command.
    Command {
        /// Command name (`entity.delete`).
        command: String,
        /// Command parameters.
        params: Value,
    },
    /// Call an Engine Protocol method (`session.begin`).
    Call {
        /// Method name.
        method: String,
        /// Method parameters.
        params: Value,
    },
    /// Run an `ara` command line: `["init"]` means `ara init`.
    Cli {
        /// Arguments after `ara`.
        args: Vec<String>,
    },
}

/// One problem (or note) reported by any subsystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// Stable code; see [`codes`].
    pub code: Code,
    /// Error, warning or info.
    pub severity: Severity,
    /// Human-readable message. Names in backticks are literal identifiers.
    pub message: String,
    /// Where it happened. Boxed to keep `Result<_, Diagnostic>` small.
    #[serde(default, skip_serializing_if = "Location::is_empty")]
    pub location: Box<Location>,
    /// What it is about besides its location: entities, assets, documents, systems,
    /// render passes, C# modules. The bus is queried by these, and `ara why`
    /// follows them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<Subject>,
    /// Structured facts for tools and explainers (a reload's level and reason, a
    /// frame's contributors). Not part of the dedup key; keep it small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<Value>>,
    /// Suggested fixes, best first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    /// Creates a diagnostic with an empty location.
    pub fn new(severity: Severity, code: Code, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            severity,
            message: message.into(),
            location: Box::default(),
            subjects: Vec::new(),
            data: None,
            fixes: Vec::new(),
        }
    }

    /// Creates an error.
    pub fn error(code: Code, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }

    /// Creates a warning.
    pub fn warning(code: Code, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, message)
    }

    /// Creates an info note.
    pub fn info(code: Code, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, code, message)
    }

    /// True for [`Severity::Error`].
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Sets the file.
    pub fn file(mut self, file: impl Into<String>) -> Self {
        self.location.file = Some(file.into());
        self
    }

    /// Sets the document asset ID.
    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.location.doc = Some(doc.into());
        self
    }

    /// Sets the record ID or entity address.
    pub fn record(mut self, record: impl Into<String>) -> Self {
        self.location.record = Some(record.into());
        self
    }

    /// Sets the field path. An empty path clears it.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        let path = path.into();
        self.location.path = (!path.is_empty()).then_some(path);
        self
    }

    /// Sets line and column (1-based).
    pub fn line_col(mut self, line: u32, column: u32) -> Self {
        self.location.line = Some(line);
        self.location.column = Some(column);
        self
    }

    /// Sets the batch command index.
    pub fn command(mut self, index: usize) -> Self {
        self.location.command = Some(index);
        self
    }

    /// Fills location parts that are still unset from `base`.
    pub fn within(mut self, base: &Location) -> Self {
        let loc = &mut self.location;
        loc.file = loc.file.take().or_else(|| base.file.clone());
        loc.doc = loc.doc.take().or_else(|| base.doc.clone());
        loc.record = loc.record.take().or_else(|| base.record.clone());
        loc.path = loc.path.take().or_else(|| base.path.clone());
        loc.command = loc.command.or(base.command);
        self
    }

    /// Adds a fix.
    pub fn with_fix(mut self, title: impl Into<String>, action: FixAction) -> Self {
        self.fixes.push(Fix {
            title: title.into(),
            action,
        });
        self
    }

    /// Adds a subject (once).
    pub fn subject(mut self, subject: Subject) -> Self {
        if !self.subjects.contains(&subject) {
            self.subjects.push(subject);
        }
        self
    }

    /// Attaches structured facts (`data`).
    pub fn data(mut self, data: Value) -> Self {
        self.data = Some(Box::new(data));
        self
    }

    /// Emits the diagnostic onto the [`bus`] (if a consumer is attached and the
    /// code is not being sampled out). Returns its event ID for causal links.
    pub fn emit(self) -> Option<bus::EventId> {
        bus::emit_with(self, bus::Meta::default())
    }

    /// Emits with metadata: a cause, a dedup key, an origin or a tick.
    pub fn emit_with(self, meta: bus::Meta) -> Option<bus::EventId> {
        bus::emit_with(self, meta)
    }
}

/// True if any diagnostic is an error.
pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(Diagnostic::is_error)
}

/// Returns the closest candidate within a small edit distance, for "did you mean" fixes.
pub fn did_you_mean<'a>(
    input: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let limit = (input.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .map(|c| (edit_distance(&input.to_lowercase(), &c.to_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, c)| (*d, *c))
        .map(|(_, c)| c)
}

/// Every candidate tied for the closest within the [`did_you_mean`] distance, in
/// order. More than one means the guess is ambiguous: offer each, apply none.
pub fn nearest<'a>(input: &str, candidates: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let limit = (input.chars().count() / 3).max(1);
    let scored: Vec<(usize, &str)> = candidates
        .into_iter()
        .map(|c| (edit_distance(&input.to_lowercase(), &c.to_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .collect();
    let best = scored.iter().map(|(d, _)| *d).min();
    scored
        .into_iter()
        .filter(|(d, _)| Some(*d) == best)
        .map(|(_, c)| c)
        .collect()
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_names_only() {
        let fields = ["position", "rotation", "scale"];
        assert_eq!(did_you_mean("positon", fields), Some("position"));
        assert_eq!(did_you_mean("Scale", fields), Some("scale"));
        assert_eq!(did_you_mean("velocity", fields), None);
    }

    #[test]
    fn ties_are_all_nearest() {
        let prefabs = ["p/crate.prefab", "p/lamp.prefab", "p/door.prefab"];
        assert_eq!(
            nearest("p/lantern.prefab", prefabs),
            ["p/crate.prefab", "p/lamp.prefab"]
        );
        assert_eq!(nearest("p/lmap.prefab", prefabs), ["p/lamp.prefab"]);
        assert!(nearest("p/vehicle_wheel.prefab", prefabs).is_empty());
    }

    #[test]
    fn json_shape_is_stable() {
        let d = Diagnostic::error(codes::UNKNOWN_FIELD, "unknown field `positon`")
            .file("levels/a.scene")
            .record("e_8b21")
            .path("Transform.positon")
            .with_fix(
                "Rename to `position`",
                FixAction::Rename {
                    record: Some("e_8b21".into()),
                    path: "Transform.positon".into(),
                    to: "position".into(),
                },
            );
        let json = serde_json::to_string(&d).expect("diagnostics serialize");
        assert_eq!(
            json,
            r#"{"code":"ARA1101","severity":"error","message":"unknown field `positon`","location":{"file":"levels/a.scene","record":"e_8b21","path":"Transform.positon"},"fixes":[{"title":"Rename to `position`","action":"rename","record":"e_8b21","path":"Transform.positon","to":"position"}]}"#
        );
        let back: Diagnostic = serde_json::from_str(&json).expect("diagnostics deserialize");
        assert_eq!(back, d);
    }
}
