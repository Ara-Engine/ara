//! Typed replacement of source bytes, with optimistic content-hash preconditions.
use crate::{Command, EditCtx};
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::{FileKind, FileState, check_path};
use ara_schema::Schema;
use serde::Deserialize;
use serde_json::{Value, json};

/// Parameters of source.replace.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceReplaceParams {
    /// Canonical project-relative C# source path.
    pub path: String,
    /// SHA-256 of the exact bytes that were analyzed.
    pub base_hash: String,
    /// Complete replacement source; line endings are preserved exactly.
    pub text: String,
}

/// Replace source through the ordinary attributed, reversible transaction history.
pub struct SourceReplace;
impl Command for SourceReplace {
    type Params = SourceReplaceParams;
    type Output = Value;
    const NAME: &'static str = "source.replace";
    const SUMMARY: &'static str =
        "Replace C# source if its base hash still matches, with offline undo and redo.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"path":"scripts/Game/Door.cs", "baseHash":FileState::bytes(b"before\n").hash,"text":"after\n"}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        check_path(&p.path)?;
        if !p.path.ends_with(".cs") {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "source.replace requires a .cs path",
            ));
        }
        let before = ctx
            .file_state(&p.path)
            .filter(|s| s.kind == FileKind::File && s.hash == p.base_hash)
            .ok_or_else(|| {
                Diagnostic::error(
                    codes::STALE_BASE,
                    format!(
                        "source `{}` changed since analysis; analyze it again",
                        p.path
                    ),
                )
                .file(&p.path)
            })?;
        let next = FileState::bytes(p.text.as_bytes());
        if before != &next {
            ctx.set_file(&p.path, Some(next.clone()))?;
        }
        Ok(json!({"path":p.path,"hash":next.hash}))
    }
}
