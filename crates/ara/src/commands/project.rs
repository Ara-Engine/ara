//! Project maintenance commands operate offline and expose a read-only migration plan.
use std::path::Path;

use ara_diag::Diagnostic;
use ara_host::project_layout::Migration;

/// Prints the plan, applying it only when explicitly requested without `--dry-run`.
pub fn migrate(
    root: &Path,
    support: Option<&Path>,
    dry_run: bool,
    json: bool,
) -> Result<bool, Diagnostic> {
    let migration = Migration::plan(root, support)?;
    if !dry_run {
        migration.apply()?;
    }
    if json {
        let mut value = serde_json::to_value(&migration)
            .expect("migration consists of serializable paths and operations");
        value["dryRun"] = dry_run.into();
        print!("{}", ara_json::to_canonical_string(&value));
    } else {
        println!(
            "{} project layout: {}",
            if dry_run { "Planned" } else { "Migrated" },
            migration.root.display()
        );
        for operation in &migration.operations {
            match operation {
                ara_host::project_layout::MigrationOp::Move { from, to } => {
                    println!("  move {} -> {}", from.display(), to.display())
                }
                ara_host::project_layout::MigrationOp::Rewrite { path } => {
                    println!("  rewrite {}", path.display())
                }
            }
        }
        if migration.operations.is_empty() {
            println!("  already current");
        }
    }
    Ok(true)
}
