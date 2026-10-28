//! The only migration writer: a dedicated metadata authority and one typed batch.
mod inventory;
mod service;
use super::{Migration, MigrationOp};
use ara_diag::{Diagnostic, codes};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

/// One project-relative change shown before consent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum MigrationOperation {
    /// Existing bytes retain their identity at a new storage path.
    Move { from: String, to: String },
    /// Authored references or project policy are rewritten canonically.
    Rewrite { path: String },
}

/// Read-only preview, bound to the exact source inventory by its consent token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationPlan {
    /// Whether ordinary project open requires this migration.
    pub needs_migration: bool,
    /// Declared old layout, or zero for pre-versioned projects.
    pub from_version: u64,
    /// Current on-disk contract version.
    pub to_version: u64,
    /// Final freely organized authored root.
    pub authored_root: String,
    /// Immutable snapshot token; any changed source requires another preview.
    pub plan_token: String,
    /// Ordered changes with project-relative paths.
    pub operations: Vec<MigrationOperation>,
    /// Final project-relative paths, excluding engine/Git state and build output.
    pub preview_tree: Vec<String>,
}

/// Cooperative cancellation ends at the typed commit boundary.
#[derive(Debug, Clone, Default)]
pub struct MigrationControl(Arc<AtomicU8>);
impl MigrationControl {
    /// Cancel preparation. False means mutation has begun or the job already ended.
    pub fn cancel(&self) -> bool {
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    /// Whether cancellation can still prevent all project changes.
    pub fn cancellable(&self) -> bool {
        self.0.load(Ordering::Acquire) == 0
    }
    /// Whether the physical, noncancellable commit is in progress.
    pub fn is_committing(&self) -> bool {
        self.0.load(Ordering::Acquire) == 2
    }
    fn check(&self) -> Result<(), Vec<Diagnostic>> {
        if self.0.load(Ordering::Acquire) == 1 {
            Err(vec![Diagnostic::error(
                codes::INVALID_PARAMS,
                "project migration cancelled before commit",
            )])
        } else {
            Ok(())
        }
    }
    fn commit(&self) -> Result<(), Vec<Diagnostic>> {
        self.0
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| {
                vec![Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "project migration cancelled before commit",
                )]
            })
    }
}

/// Inspect a closed legacy project without locks, cache writes or profile access.
pub fn migration_plan(root: &Path) -> Result<MigrationPlan, Vec<Diagnostic>> {
    let migration = Migration::plan(root, None).map_err(|d| vec![d])?;
    Ok(inventory::prepare(&migration, false)?.plan)
}

/// Worker-only entry point shared by CLI, Hub and editor. It opens a dedicated
/// authority, submits human-attributed cmd.apply, then waits for its file receipt.
/// Ordinary Project::open is never used for a legacy project.
pub fn migrate_project(
    root: &Path,
    plan_token: &str,
    control: &MigrationControl,
    progress: &mut dyn FnMut(f32, &str),
) -> Result<Value, Vec<Diagnostic>> {
    let result = (|| {
        control.check()?;
        progress(0.0, "Checking the reviewed migration");
        let migration = Migration::plan(root, None).map_err(|d| vec![d])?;
        let preview = inventory::prepare(&migration, false)?;
        if preview.plan.plan_token != plan_token {
            return Err(vec![Diagnostic::error(
                codes::STALE_BASE,
                "project changed since preview; review the migration again",
            )]);
        }
        if !preview.plan.needs_migration {
            return Ok(json!({"migrated":false,"restartRequired":false,"planToken":plan_token}));
        }
        control.check()?;
        let project = crate::Project::migration_bootstrap(&migration.root).map_err(|d| vec![d])?;
        let mut host = crate::Host::open_migration(
            project,
            crate::HostConfig {
                watch_every: std::time::Duration::from_secs(86_400),
                scan_every: std::time::Duration::from_secs(86_400),
                ..crate::HostConfig::default()
            },
        )
        .map_err(|d| vec![d])?;
        progress(0.15, "Preparing reversible file snapshots");
        let current = Migration::plan(&migration.root, None).map_err(|d| vec![d])?;
        let prepared = inventory::prepare(&current, true)?;
        if prepared.plan.plan_token != plan_token {
            return Err(vec![Diagnostic::error(
                codes::STALE_BASE,
                "project changed while opening migration authority; review again",
            )]);
        }
        control.check()?;
        crate::file_state::FileHistory::new(&migration.root)
            .revalidate(&prepared.inventory)
            .map_err(|d| vec![d])?;
        let completion = Arc::new(std::sync::Mutex::new(None));
        host.register(Box::new(service::MigrationService::new(
            prepared,
            control.clone(),
            completion.clone(),
        )))
        .map_err(|d| vec![d])?;
        let session=host.call(ara_proto::ConnId::LOCAL,"session.begin",json!({"client":"ara-project-migration","author":"human","label":"Migrate project layout"})).map_err(|error|error.diagnostics())?;
        let session = session["session"].as_str().ok_or_else(|| {
            vec![Diagnostic::error(
                codes::INVALID_PARAMS,
                "migration session was not created",
            )]
        })?;
        progress(0.4, "Applying the reviewed project migration");
        host.call(ara_proto::ConnId::LOCAL,"cmd.apply",json!({"session":session,"doc":"ara.project.json","commands":[{"cmd":"project.migrate","args":{"planToken":plan_token}}],"why":"Migrate project to the reviewed preview-4 layout"})).map_err(|error|error.diagnostics())?;
        loop {
            host.tick();
            if let Some(result) = completion
                .lock()
                .map_err(|_| {
                    vec![Diagnostic::error(
                        codes::IO,
                        "migration completion channel failed",
                    )]
                })?
                .take()
            {
                progress(1.0, "Project migration saved");
                return result.map(|mut value| {
                    value["engineVersion"] = super::CURRENT_ENGINE_VERSION.into();
                    value
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    })();
    control.0.store(3, Ordering::Release);
    result
}

pub(super) fn apply_legacy_plan(migration: &Migration) -> Result<(), Diagnostic> {
    if migration.support != migration.root.join("migration-support") {
        return Err(Diagnostic::error(
            codes::INVALID_PARAMS,
            "typed migration retains support under migration-support/; custom support destinations are unsupported",
        ));
    }
    let prepared = inventory::prepare(migration, false).map_err(first)?;
    migrate_project(
        &migration.root,
        &prepared.plan.plan_token,
        &MigrationControl::default(),
        &mut |_, _| {},
    )
    .map(|_| ())
    .map_err(first)
}
fn first(errors: Vec<Diagnostic>) -> Diagnostic {
    errors
        .into_iter()
        .next()
        .expect("failed migration has a diagnostic")
}
