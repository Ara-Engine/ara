//! Explicitly replace a same-version registration with the signed current SDK.
use super::*;

impl Hub {
    pub(super) fn validate_current_install(
        &self,
        version: &str,
    ) -> Result<Option<EngineBuild>, RpcError> {
        let destination = self.updates.config.engines.join(version);
        self.ensure_unused(version, &destination)?;
        if self.updates.jobs.values().any(|job| {
            job.version == version && job.state == "running" && job.kind != "engine-current"
        }) {
            return Err(invalid(
                "wait for this engine version's active operation before installing the current SDK",
            ));
        }
        let previous = self
            .registry
            .engines
            .iter()
            .find(|engine| engine.version == version)
            .cloned();
        if destination.exists() {
            uninstall::validate_managed(&destination, version)?;
            if !previous
                .as_ref()
                .is_some_and(|engine| same_executable(engine, &destination))
            {
                return Err(invalid(
                    "the current SDK destination belongs to another installation; it was preserved",
                ));
            }
        }
        Ok(previous)
    }
}

pub(super) fn validate_staged(
    staged: &Path,
    directory: &Path,
    cancel: &Cancel,
) -> Result<(), ara_update::Error> {
    cancel.check()?;
    if !super::super::capabilities::query(&staged.join("ara.exe"), directory)
        .is_some_and(|layouts| layouts.contains(&4))
    {
        return Err(ara_update::Error {code:ara_diag::codes::PROJECT_ENGINE_LAYOUT_UNSUPPORTED,message:"The downloaded SDK does not declare support for the content/ layout; the existing engine was preserved".into()});
    }
    cancel.check()
}

pub(super) fn same_registration(left: Option<&EngineBuild>, right: Option<&EngineBuild>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.version == right.version
                && left.executable == right.executable
                && left.editor == right.editor
        }
        _ => false,
    }
}
