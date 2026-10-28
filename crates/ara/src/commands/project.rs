//! Project maintenance submits typed commands through the dedicated migration worker.
use ara_diag::{Diagnostic, codes};
use ara_host::project_layout::{MigrationControl, migrate_project, migration_plan};
use std::path::Path;

/// Preview without creating state, or explicitly apply one journaled migration.
pub fn migrate(
    root: &Path,
    support: Option<&Path>,
    dry_run: bool,
    json: bool,
) -> Result<bool, Diagnostic> {
    if support.is_some() {
        return Err(Diagnostic::error(
            codes::INVALID_PARAMS,
            "migration support is retained inside migration-support/ so Undo can restore it",
        ));
    }
    let plan = migration_plan(root).map_err(first)?;
    let result = if dry_run {
        None
    } else {
        Some(
            migrate_project(
                root,
                &plan.plan_token,
                &MigrationControl::default(),
                &mut |_, _| {},
            )
            .map_err(first)?,
        )
    };
    if json {
        let mut value = serde_json::to_value(&plan).expect("migration plan is serializable");
        value["dryRun"] = dry_run.into();
        if let Some(result) = result {
            value["result"] = result;
        }
        print!("{}", ara_json::to_canonical_string(&value));
    } else {
        println!(
            "{} project layout: {}",
            if dry_run { "Planned" } else { "Migrated" },
            root.display()
        );
        for operation in plan.operations {
            match operation {
                ara_host::project_layout::MigrationOperation::Move { from, to } => {
                    println!("  move {from} -> {to}")
                }
                ara_host::project_layout::MigrationOperation::Rewrite { path } => {
                    println!("  rewrite {path}")
                }
            }
        }
        if !plan.needs_migration {
            println!("  already current");
        }
    }
    Ok(true)
}
fn first(errors: Vec<Diagnostic>) -> Diagnostic {
    errors
        .into_iter()
        .next()
        .expect("failed migration has a diagnostic")
}
