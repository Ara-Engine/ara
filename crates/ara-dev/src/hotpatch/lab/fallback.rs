//! Preserve the caller's CPU budget when optional DX selects ordinary warm reload.
use crate::{
    binaries::Binaries,
    lab::build::{ReloadStrategy, WarmRestart},
};
use anyhow::Result;
use std::{path::Path, process::Command};

pub(super) struct Warm;
impl ReloadStrategy for Warm {
    fn name(&self) -> &'static str {
        WarmRestart.name()
    }
    fn reuse_unchanged_player(&self) -> bool {
        true
    }
    fn publish_superseded(&self) -> bool {
        true
    }
    fn command(&self, root: &Path, target: &Path) -> Command {
        let mut command = WarmRestart.command(root, target);
        command.env("CARGO_BUILD_JOBS", super::super::tool::build_jobs());
        command
    }
    fn stage(&self, directory: &Path, cache: &Path) -> Result<Binaries> {
        WarmRestart.stage(directory, cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_keeps_the_callers_budget_and_normal_reuse_policy() {
        let command = Warm.command(Path::new("root"), Path::new("target"));
        let jobs = command
            .get_envs()
            .find(|(name, _)| *name == "CARGO_BUILD_JOBS")
            .and_then(|(_, value)| value);
        assert_eq!(
            jobs,
            Some(super::super::super::tool::build_jobs().as_os_str())
        );
        assert!(Warm.reuse_unchanged_player() && Warm.publish_superseded());
        assert_eq!(Warm.name(), "rebuild_warm_restart");
    }
}
