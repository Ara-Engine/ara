//! A migration is one prepared transaction; filesystem bytes remain host-owned.
use crate::{Command, EditCtx};
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Edit, RecordId, file_state::FileState};
use ara_schema::{FieldPath, Schema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Metadata owner for a fully observed migration plan.
pub const PROJECT_MIGRATION_METADATA: &str = "ara.projectMigration";

/// Consent to the exact read-only migration preview.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectMigrateParams {
    /// Immutable token returned by the migration preview; changed files require review again.
    pub plan_token: String,
}

/// One reversible project-relative transition prepared by the host worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationFile {
    /// Canonical project-relative storage path.
    pub path: String,
    /// Exact initial bytes, directory or absence.
    pub before: Option<FileState>,
    /// Exact final bytes, directory or absence.
    pub after: Option<FileState>,
}

/// Immutable facts checked by the worker and consumed by the pure command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedMigration {
    /// Preview consent token.
    pub plan_token: String,
    /// Initial marker value, preventing use against another project authority.
    pub marker_before: Value,
    /// Final marker value.
    pub marker_after: Value,
    /// Parent creates, file transitions and child-first directory removals.
    pub files: Vec<MigrationFile>,
}

/// Move authored files and rewrite references as one undoable command.
pub struct ProjectMigrate;

impl Command for ProjectMigrate {
    type Params = ProjectMigrateParams;
    type Output = Value;
    const NAME: &'static str = "project.migrate";
    const SUMMARY: &'static str =
        "Apply an explicitly reviewed project-layout migration as one reversible transaction.";
    const SCOPE: &'static str = "edit:assets";

    fn examples() -> Vec<Value> {
        vec![json!({"planToken":"reviewed-inventory-token"})]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        let metadata = ctx
            .prepared_metadata()
            .and_then(|value| value.get(PROJECT_MIGRATION_METADATA))
            .ok_or_else(|| {
                Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "project migration requires its dedicated preparation authority",
                )
            })?;
        let prepared: PreparedMigration = serde_json::from_value(metadata.clone())
            .map_err(|error| Diagnostic::error(codes::INVALID_PARAMS, error.to_string()))?;
        if prepared.plan_token != params.plan_token {
            return Err(Diagnostic::error(
                codes::STALE_BASE,
                "migration plan changed; review a new preview before migrating",
            ));
        }
        let marker = ctx
            .read()
            .workspace()
            .doc("pj_project")
            .and_then(|doc| doc.record("p_settings"))
            .and_then(|record| record.get("settings"))
            .cloned();
        if marker.as_ref() != Some(&prepared.marker_before) {
            return Err(Diagnostic::error(
                codes::STALE_BASE,
                "project marker changed since migration was prepared",
            ));
        }
        for file in &prepared.files {
            ara_doc::file_state::check_path(&file.path)?;
            if ctx.file_state(&file.path) != file.before.as_ref() {
                return Err(Diagnostic::error(
                    codes::STALE_BASE,
                    "migration file inventory changed",
                )
                .file(&file.path));
            }
        }
        // Marker policy is also a native op, so Undo immediately invalidates the open layout.
        ctx.apply_edit(Edit::Set {
            doc: AssetId::parse("pj_project").expect("fixed project ID"),
            id: RecordId::parse("p_settings").expect("fixed project settings ID"),
            path: FieldPath::field("settings"),
            value: Some(prepared.marker_after),
        })?;
        let count = prepared.files.len();
        for file in prepared.files {
            ctx.set_file(&file.path, file.after)?;
        }
        Ok(
            json!({"migrated":true,"planToken":params.plan_token,"files":count,"restartRequired":true}),
        )
    }
}
