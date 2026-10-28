//! Shared intermediates reuse Cargo fingerprints without sharing final executable names.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

/// Gate builds are usually cold after disk cleanup. Avoid multi-GB incremental
/// writes, which also make Rust libraries ineligible for the optional sccache.
#[derive(Serialize)]
pub(crate) struct GatePolicy {
    pub(crate) incremental: String,
    pub(crate) cache: &'static str,
    #[serde(skip)]
    wrapper: Option<OsString>,
}

impl GatePolicy {
    pub(crate) fn from_env(bypass: bool) -> Self {
        Self::new(
            bypass,
            std::env::var_os("ARA_SHARED_BUILD_DIR").is_some_and(|v| !v.is_empty()),
            std::env::var_os("CARGO_INCREMENTAL")
                .or_else(|| std::env::var_os("CARGO_BUILD_INCREMENTAL")),
            std::env::var_os("RUSTC_WRAPPER"),
            optional_sccache(),
        )
    }

    fn new(
        bypass: bool,
        shared: bool,
        incremental: Option<OsString>,
        wrapper: Option<OsString>,
        installed: Option<OsString>,
    ) -> Self {
        let incremental =
            incremental.map_or_else(|| "0".into(), |v| v.to_string_lossy().into_owned());
        let (cache, wrapper) = if bypass || shared {
            (
                if shared {
                    "shared Cargo fingerprints"
                } else {
                    "disabled by flag"
                },
                Some(OsString::new()),
            )
        } else if incremental == "1"
            && (wrapper.as_ref().is_some_and(|v| is_sccache(v))
                || (wrapper.is_none() && installed.is_some()))
        {
            // sccache 0.18 exits before compilation when CARGO_INCREMENTAL=1.
            // Preserve the requested probe/debug mode using Cargo's own cache.
            ("incremental requires direct rustc", Some(OsString::new()))
        } else if let Some(wrapper) = wrapper {
            (
                if wrapper.is_empty() {
                    "disabled by environment"
                } else {
                    "caller wrapper"
                },
                Some(wrapper),
            )
        } else if let Some(installed) = installed {
            ("optional sccache", Some(installed))
        } else {
            ("Cargo configured wrapper or direct rustc", None)
        };
        Self {
            incremental,
            cache,
            wrapper,
        }
    }

    pub(crate) fn bypassed(&self) -> bool {
        self.wrapper.as_ref().is_some_and(|v| v.is_empty())
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        command.env("CARGO_INCREMENTAL", &self.incremental);
        if let Some(wrapper) = &self.wrapper {
            command.env("RUSTC_WRAPPER", wrapper);
        }
        // Retry mid-compile server I/O loss through the cache's direct compiler
        // fallback. Startup/configuration errors remain actionable (--no-compiler-cache).
        if std::env::var_os("SCCACHE_IGNORE_SERVER_IO_ERROR").is_none() {
            command.env("SCCACHE_IGNORE_SERVER_IO_ERROR", "1");
        }
    }
}

fn optional_sccache() -> Option<OsString> {
    let name = format!("sccache{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(&name))
        .find(|path| path.is_file())
        .map(PathBuf::into_os_string)
}

fn is_sccache(value: &std::ffi::OsStr) -> bool {
    Path::new(value)
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("sccache"))
}

pub(crate) struct Lease {
    _file: File,
}

/// Hold this through execution too: another branch must not replace live test dependencies.
pub(crate) fn acquire(deadline: Option<Instant>) -> Result<Option<Lease>> {
    let Some(path) = std::env::var_os("ARA_SHARED_BUILD_DIR").filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let directory = PathBuf::from(path);
    Ok(Some(acquire_at(&directory, deadline)?))
}

fn acquire_at(directory: &Path, deadline: Option<Instant>) -> Result<Lease> {
    fs::create_dir_all(directory)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".ara-build-admission.lock"))?;
    let start = Instant::now();
    let mut announced = false;
    loop {
        match file.try_lock() {
            Ok(()) => {
                eprintln!(
                    "Shared build admission: {:.2}s wait; {}",
                    start.elapsed().as_secs_f64(),
                    directory.display()
                );
                return Ok(Lease { _file: file });
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if deadline.is_some_and(|limit| Instant::now() >= limit) {
                    bail!("shared build admission timed out at gate deadline");
                }
                if !announced {
                    eprintln!(
                        "Waiting for shared compilation/test lane: {}",
                        directory.display()
                    );
                    announced = true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Cargo fingerprints are the shared cache. Avoid the independent sccache jobserver
/// in this lane; rustc inherits Cargo's requested budget (two by default).
pub(crate) fn configure(command: &mut Command) {
    configure_with(
        command,
        std::env::var_os("CARGO_BUILD_JOBS"),
        std::env::var_os("ARA_SHARED_BUILD_DIR").is_some(),
    );
}

fn configure_with(command: &mut Command, jobs: Option<OsString>, shared: bool) {
    // Preserve explicit values verbatim: Cargo validates its own job syntax.
    command.env("CARGO_BUILD_JOBS", jobs.unwrap_or_else(|| "2".into()));
    if shared {
        command
            .env("CARGO_INCREMENTAL", "0")
            .env("RUSTC_WRAPPER", "");
    }
}

pub(crate) fn cargo(root: &Path, args: &[OsString]) -> Result<bool> {
    if let Some(result) = crate::editor::cargo(root, args)? {
        return Ok(result);
    }
    let directories = crate::build_paths::Directories::from_env(root)?;
    let _lease = acquire(Some(Instant::now() + Duration::from_secs(2400)))?;
    let mut command = Command::new("cargo");
    command.current_dir(root).args(args);
    directories.apply(&mut command);
    configure(&mut command);
    eprintln!("Focused cargo {args:?}");
    Ok(crate::process::run(&mut command, None)
        .context("focused cargo")?
        .success())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gate_cache_is_optional_and_nonincremental_without_changing_focused_defaults() {
        let installed = Some(OsString::from("available/sccache"));
        let auto = GatePolicy::new(false, false, None, None, installed.clone());
        assert_eq!(auto.incremental, "0");
        assert_eq!(auto.cache, "optional sccache");
        assert!(!auto.bypassed());
        let mut gate = Command::new("cargo");
        auto.apply(&mut gate);
        assert_eq!(
            gate.get_envs()
                .find(|(name, _)| *name == "CARGO_INCREMENTAL")
                .and_then(|(_, v)| v),
            Some(std::ffi::OsStr::new("0"))
        );
        let absent = GatePolicy::new(false, false, None, None, None);
        assert!(absent.wrapper.is_none(), "do not require installing a tool");
        let configured = GatePolicy::new(false, false, Some("1".into()), None, None);
        assert!(
            configured.wrapper.is_none(),
            "preserve Cargo's configured wrapper when no sccache is selected"
        );
        assert_eq!(configured.incremental, "1");
        assert_eq!(configured.cache, "Cargo configured wrapper or direct rustc");
        let incremental = GatePolicy::new(false, false, Some("1".into()), None, installed.clone());
        assert!(
            incremental.bypassed(),
            "sccache rejects an explicit incremental build"
        );
        for (bypass, shared) in [(true, false), (false, true)] {
            let policy = GatePolicy::new(bypass, shared, Some("1".into()), None, installed.clone());
            assert!(policy.bypassed());
            assert_eq!(
                policy.incremental, "1",
                "preserve explicit measurement override"
            );
        }
        for wrapper in [OsString::new(), "custom-wrapper".into()] {
            let policy =
                GatePolicy::new(false, false, None, Some(wrapper.clone()), installed.clone());
            assert_eq!(policy.wrapper, Some(wrapper));
        }
        let mut focused = Command::new("cargo");
        configure_with(&mut focused, None, false);
        assert!(
            !focused
                .get_envs()
                .any(|(key, _)| key == "CARGO_INCREMENTAL")
        );
    }
    #[test]
    fn explicit_build_jobs_survive_both_cache_modes_and_unset_defaults_to_two() {
        for shared in [false, true] {
            for (requested, expected) in [(None, "2"), (Some("4"), "4"), (Some("1"), "1")] {
                let mut command = Command::new("cargo");
                configure_with(&mut command, requested.map(OsString::from), shared);
                assert_eq!(
                    command
                        .get_envs()
                        .find(|(key, _)| *key == "CARGO_BUILD_JOBS")
                        .and_then(|(_, value)| value),
                    Some(std::ffi::OsStr::new(expected))
                );
            }
        }
    }
    #[test]
    fn shared_artifacts_remain_exclusive_until_execution_releases_the_lease() {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let directory = target.join(format!("ara-dev/admission-{}", std::process::id()));
        let first = acquire_at(&directory, None).unwrap();
        assert!(
            acquire_at(
                &directory,
                Some(Instant::now() + Duration::from_millis(100))
            )
            .is_err()
        );
        drop(first);
        assert!(acquire_at(&directory, Some(Instant::now() + Duration::from_secs(1))).is_ok());
    }
}
