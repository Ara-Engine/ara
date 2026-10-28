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
    /// SHA-256 of the exact bytes that were analyzed; null creates an absent source.
    pub base_hash: Option<String>,
    /// Existing C# source to relocate while replacing it; destination must be absent
    /// or the same file under different filesystem casing.
    pub from: Option<String>,
    /// Complete replacement source; line endings are preserved exactly.
    pub text: String,
}

/// Replace source through the ordinary attributed, reversible transaction history.
pub struct SourceReplace;
impl Command for SourceReplace {
    type Params = SourceReplaceParams;
    type Output = Value;
    const NAME: &'static str = "source.replace";
    const SUMMARY: &'static str = "Create, replace or relocate C# source at its checked base hash, with offline undo and redo.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"path":"scripts/Game/Door.cs", "baseHash":FileState::bytes(b"before\n").hash,"text":"after\n"}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        check_path(&p.path)?;
        if !p
            .path
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("cs"))
        {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "source.replace requires a .cs path",
            ));
        }
        let from = p.from.as_deref().unwrap_or(&p.path);
        check_path(from)?;
        if !from
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("cs"))
        {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "source.replace requires a .cs source path",
            ));
        }
        let before = ctx.file_state(from).cloned();
        if before.as_ref().is_some_and(|s| s.kind != FileKind::File)
            || before.as_ref().map(|s| s.hash.as_str()) != p.base_hash.as_deref()
        {
            return Err(Diagnostic::error(
                codes::STALE_BASE,
                format!("source `{}` changed since analysis; analyze it again", from),
            )
            .file(from));
        }
        let relocating = from != p.path;
        if relocating && before.is_none() {
            return Err(
                Diagnostic::error(codes::STALE_BASE, "source to relocate is absent").file(from),
            );
        }
        if (before.is_none() || relocating)
            && !p
                .path
                .split('/')
                .next()
                .is_some_and(|part| ara_doc::file_state::same_path(part, ara_doc::AUTHORING_ROOT))
        {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                format!("new C# sources must be under {}/", ara_doc::AUTHORING_ROOT),
            )
            .file(&p.path));
        }
        if relocating
            && !ara_doc::file_state::same_path(from, &p.path)
            && ctx.file_state(&p.path).is_some()
        {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "source destination already exists",
            )
            .file(&p.path));
        }
        let next = FileState::bytes(p.text.as_bytes());
        if before.as_ref() != Some(&next) || relocating {
            if before.is_none() || relocating {
                let mut parents = Vec::new();
                let mut path = p.path.as_str();
                while let Some((parent, _)) = path.rsplit_once('/') {
                    parents.push(parent);
                    path = parent;
                }
                for parent in parents.into_iter().rev() {
                    if ctx.file_state(parent).is_none() {
                        ctx.set_file(parent, Some(FileState::directory()))?;
                    }
                }
            }
            if relocating {
                ctx.set_file(from, None)?;
            }
            ctx.set_file(&p.path, Some(next.clone()))?;
        }
        Ok(json!({"path":p.path,"hash":next.hash}))
    }
}
