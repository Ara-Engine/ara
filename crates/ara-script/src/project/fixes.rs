//! Installed analyzer code fixes over immutable source text. Authority owns all writes.
use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

/// Source bytes decoded as UTF-8 for the installed Roslyn tool.
#[derive(Debug, Serialize)]
pub struct SourceText {
    /// Project-relative storage path; returned unchanged with each replacement.
    pub path: String,
    /// Exact source version that was analyzed.
    pub text: String,
}
/// A shipped code-fix action that contributed to a replacement.
#[derive(Debug, Deserialize, Serialize)]
pub struct AppliedFix {
    /// Analyzer diagnostic code.
    pub code: String,
    /// Provider's human-readable action title.
    pub title: String,
}
/// One proposed source replacement. Commit it through source.replace, using before's hash.
#[derive(Debug, Deserialize, Serialize)]
pub struct SourceChange {
    /// Original project-relative storage path.
    pub path: String,
    /// Exact analyzed source.
    pub before: String,
    /// Replacement text produced by the installed providers.
    pub after: String,
    /// Applied actions in provider execution order.
    pub fixes: Vec<AppliedFix>,
}

/// Run the SDK's real ARA0001/ARA0101/ARA0506 providers, without writing source files.
/// Empty codes selects all shipped fixes; the folder-derived profile controls analysis.
pub fn code_fixes(
    root: &Path,
    files: &[SourceText],
    codes: &[String],
    profile: crate::ModuleProfile,
) -> Result<Vec<SourceChange>, Diagnostic> {
    let root = &std::fs::canonicalize(root).map_err(super::io)?;
    #[derive(Deserialize)]
    struct Result {
        changes: Vec<SourceChange>,
    }
    let result: Result = super::tool::run(
        root,
        "--fix",
        json!({"files":files,"codes":(!codes.is_empty()).then_some(codes),"profile":profile.as_str()}),
    )?;
    Ok(result.changes)
}
