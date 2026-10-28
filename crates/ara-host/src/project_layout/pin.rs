//! Explicit engine-pin changes use native ops without discovering or compiling game code.
use super::{CURRENT_ENGINE_VERSION, migration_plan, require_current_layout};
use ara_diag::{Diagnostic, codes};
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

/// Pin a closed current-layout project to the current capable engine as one
/// human-attributed transaction, waiting for physical persistence before success.
pub fn pin_current_engine(root: &Path) -> Result<Value, Vec<Diagnostic>> {
    require_current_layout(root).map_err(|error| vec![error])?;
    let preview = migration_plan(root)?;
    let project = crate::Project::migration_bootstrap(root).map_err(|error| vec![error])?;
    let mut host = crate::Host::open_migration(
        project,
        crate::HostConfig {
            watch_every: Duration::from_secs(86_400),
            scan_every: Duration::from_secs(86_400),
            ..crate::HostConfig::default()
        },
    )
    .map_err(|error| vec![error])?;
    if migration_plan(root)?.plan_token != preview.plan_token {
        return Err(vec![Diagnostic::error(
            codes::STALE_BASE,
            "project changed while preparing its engine pin; retry the explicit engine change",
        )]);
    }
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"ara-project-engine","author":"human","label":"Use current engine"}),
        )
        .map_err(|error| error.diagnostics())?;
    let result = host.call(ConnId::LOCAL,"cmd.apply",json!({"session":session["session"],"doc":"ara.project.json","why":"Use the current engine for this project layout","commands":[{"cmd":"property.set","args":{"entity":"p_settings","path":"settings.engineVersion","value":CURRENT_ENGINE_VERSION}}]}))
        .map_err(|error| error.diagnostics())?;
    let receipt = host.queue_flush();
    loop {
        host.tick();
        if let Some(saved) = host.poll_flush(receipt) {
            saved?;
            return Ok(
                json!({"engineVersion":CURRENT_ENGINE_VERSION,"transaction":result["txn"],"saved":true}),
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
