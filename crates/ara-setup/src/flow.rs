//! The install transaction shared by the window and the silent command line:
//! verify the payload, inspect the destination, extract into a stage, publish it
//! (or swap it in with rollback), then register through the package's `install.ps1`.
use crate::{
    args::Args,
    processes::{self, Blocker},
    register,
};
use anyhow::{Context, Result, bail};
use ara_export_installer::{Payload, Stage, rename_retrying};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Setup version; release packaging requires the payload to match it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a payload installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A versioned engine build (`engine.json`).
    Engine,
    /// The standalone launcher (`hub.json`).
    Hub,
}

impl Kind {
    /// Recognizes a payload by its manifest.
    pub fn of(names: &[String]) -> Result<Self> {
        if names.iter().any(|name| name == "hub.json") {
            Ok(Self::Hub)
        } else if names.iter().any(|name| name == "engine.json") {
            Ok(Self::Engine)
        } else {
            bail!(
                "ARA-EXP-INSTALL-001: the payload is neither an Ara engine nor an Ara Hub package"
            )
        }
    }
    /// What people call it: `Ara 0.0.1-alpha` or `Ara Hub`.
    pub fn product(self) -> String {
        match self {
            Self::Engine => format!("Ara {VERSION}"),
            Self::Hub => "Ara Hub".to_owned(),
        }
    }
    /// `%LOCALAPPDATA%\Programs\Ara\<version>` or `%LOCALAPPDATA%\Programs\Ara Hub`.
    pub fn default_destination(self) -> Result<PathBuf> {
        let programs =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?)
                .join("Programs");
        Ok(match self {
            Self::Engine => programs.join("Ara").join(VERSION),
            Self::Hub => programs.join("Ara Hub"),
        })
    }
    /// Manifest file and format that mark an installation of this kind.
    fn marker(self) -> (&'static str, &'static str) {
        match self {
            Self::Engine => ("engine.json", "ara.engine/1"),
            Self::Hub => ("hub.json", "ara.hub/1"),
        }
    }
}

/// The destination as found before installing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Existing {
    /// Nothing there: a fresh install.
    Missing,
    /// An Ara installation of the same kind, with its recorded version.
    Install {
        /// `version` from its manifest.
        version: String,
    },
    /// Anything else, including the other kind of Ara install; never modified.
    Foreign,
}

/// Classifies `destination` for a payload of `kind` by its manifest. Links and
/// junctions are foreign: setup never replaces through them.
pub fn inspect(destination: &Path, kind: Kind) -> Existing {
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Existing::Missing,
        Err(_) => return Existing::Foreign,
    };
    let (marker, format) = kind.marker();
    let (other, _) = match kind {
        Kind::Engine => Kind::Hub,
        Kind::Hub => Kind::Engine,
    }
    .marker();
    if !metadata.is_dir() || destination.join(other).exists() {
        return Existing::Foreign;
    }
    let Ok(bytes) = fs::read(destination.join(marker)) else {
        return Existing::Foreign;
    };
    // Windows PowerShell writes UTF-8 with a BOM.
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let Ok(manifest) = serde_json::from_slice::<Value>(bytes) else {
        return Existing::Foreign;
    };
    match (manifest["format"].as_str(), manifest["version"].as_str()) {
        (Some(found), Some(version)) if found == format => Existing::Install {
            version: version.to_owned(),
        },
        _ => Existing::Foreign,
    }
}

/// What is installed where; known once the payload is verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Payload kind.
    pub kind: Kind,
    /// Absolute installation directory.
    pub destination: PathBuf,
}

/// Progress, in the order the steps happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    /// Hashing the payload: archive bytes read so far.
    Verify {
        /// Bytes hashed.
        done: u64,
        /// Archive size.
        total: u64,
    },
    /// Writing files: the current entry and expanded bytes written so far.
    Extract {
        /// Entry being written.
        file: &'a str,
        /// Expanded bytes written.
        done: u64,
        /// Expanded payload size.
        total: u64,
    },
    /// Stopping the Hub service gracefully (`ara hub shutdown`).
    StopService,
    /// Moving the previous installation aside and the new one into place.
    Swap,
    /// Deleting the replaced files.
    Cleanup,
    /// Running `install.ps1 -RegisterOnly`.
    Register,
}

/// How to continue while programs block the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Check again (the person closed them).
    Retry,
    /// Stop the Hub service through `ara hub shutdown`, then check again.
    StopService,
    /// Leave everything as it is.
    Cancel,
}

/// The person's side of a transaction: the window implements it. Silent installs
/// use [`Quiet`], whose questions are never asked.
pub trait Host {
    /// The payload is verified: what will be installed where.
    fn plan(&mut self, plan: &Plan);
    /// A step advanced.
    fn progress(&mut self, step: Step<'_>);
    /// A same-kind installation exists: true replaces it.
    fn confirm_replace(&mut self, plan: &Plan, installed: &str) -> bool;
    /// Programs block the destination. `blockers` is empty when Windows reports
    /// files in use but no program runs from the folder.
    fn resolve(&mut self, plan: &Plan, blockers: &[Blocker]) -> Resolution;
}

/// The silent host: no progress output and no questions.
pub struct Quiet;
impl Host for Quiet {
    fn plan(&mut self, _: &Plan) {}
    fn progress(&mut self, _: Step<'_>) {}
    fn confirm_replace(&mut self, _: &Plan, _: &str) -> bool {
        false
    }
    fn resolve(&mut self, _: &Plan, _: &[Blocker]) -> Resolution {
        Resolution::Cancel
    }
}

/// A finished installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// What was installed where.
    pub plan: Plan,
    /// An existing installation was replaced.
    pub replaced: bool,
    /// The replaced files, when deleting them failed.
    pub leftover: Option<PathBuf>,
    /// Registration output.
    pub output: String,
}

/// How a transaction ended without an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Installed and registered.
    Installed(Installed),
    /// The person cancelled; nothing changed.
    Cancelled,
}

/// Installs the payload appended to `exe`. Silent installs follow `args` alone;
/// otherwise `host` is asked before replacing and while programs block the folder.
pub fn run(exe: &Path, args: &Args, host: &mut dyn Host) -> Result<Outcome> {
    let payload = Payload::open(exe)?;
    let size = payload.len();
    let mut verified = payload.verify(|done| host.progress(Step::Verify { done, total: size }))?;
    let kind = Kind::of(verified.names())?;
    let destination = match &args.destination {
        Some(destination) => destination.clone(),
        None => kind.default_destination()?,
    };
    let plan = Plan {
        kind,
        destination: std::path::absolute(destination)?,
    };
    host.plan(&plan);
    let shown = plan.destination.display();
    let replacing = match inspect(&plan.destination, kind) {
        Existing::Missing => false,
        Existing::Install { version } => {
            if args.silent && !args.replace {
                bail!(
                    "ARA-EXP-INSTALL-004: destination already exists: {shown} holds {} {version}; pass --replace to replace it",
                    kind_name(kind)
                );
            }
            if !args.replace && !host.confirm_replace(&plan, &version) {
                return Ok(Outcome::Cancelled);
            }
            true
        }
        Existing::Foreign if args.silent && !args.replace => {
            bail!("ARA-EXP-INSTALL-004: destination already exists: {shown}")
        }
        Existing::Foreign => bail!(
            "ARA-EXP-INSTALL-006: {shown} exists but is not an {} installation, so setup will not change it. Choose another --destination, or move that folder yourself",
            kind_name(kind)
        ),
    };
    // The same OS lock is held by the updater and engine's Hub registration.
    let _hub_lock = if kind == Kind::Hub {
        let parent = plan.destination.parent().context("Hub install parent")?;
        fs::create_dir_all(parent)?;
        let lock = fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(parent.join(".ara-hub-update.lock"))?;
        lock.try_lock()
            .context("another Hub install/update is running")?;
        Some(lock)
    } else {
        None
    };
    if replacing {
        loop {
            let blockers = processes::running_from(&plan.destination);
            if blockers.is_empty() {
                break;
            }
            if args.silent {
                bail!(
                    "ARA-EXP-INSTALL-007: Ara programs are running from {shown}: {}. Close them and run setup again",
                    processes::describe(&blockers)
                );
            }
            if !ask(&plan, host, &blockers)? {
                return Ok(Outcome::Cancelled);
            }
        }
    }
    let stage = Stage::beside(&plan.destination)?;
    let expanded = verified.total();
    verified.extract(&stage, |file, done| {
        host.progress(Step::Extract {
            file,
            done,
            total: expanded,
        })
    })?;
    if replacing && kind == Kind::Hub && !hub_build_is_newer(stage.path(), &plan.destination)? {
        return Ok(Outcome::Installed(Installed {
            plan,
            replaced: false,
            leftover: None,
            output: "The installed Hub build is already current or newer.".into(),
        }));
    }
    host.progress(Step::Swap);
    let leftover = if replacing {
        match swap(stage, &plan, args, host)? {
            Swapped::Done { leftover } => leftover,
            Swapped::Cancelled => return Ok(Outcome::Cancelled),
        }
    } else {
        stage.publish(&plan.destination)?;
        None
    };
    host.progress(Step::Register);
    let output = register::run(&plan.destination, args.silent, args.no_system_integration)?;
    Ok(Outcome::Installed(Installed {
        plan,
        replaced: replacing,
        leftover,
        output,
    }))
}

fn hub_build_is_newer(source: &Path, destination: &Path) -> Result<bool> {
    let read = |root: &Path| -> Result<Value> {
        let bytes = fs::read(root.join("hub.json"))?;
        Ok(serde_json::from_slice(
            bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes),
        )?)
    };
    let new = read(source)?;
    let old = read(destination)?;
    let version = |v: &Value| -> Result<semver::Version> {
        Ok(v["version"].as_str().context("Hub version")?.parse()?)
    };
    Ok(match version(&new)?.cmp(&version(&old)?) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (
            new["build"]["generation"].as_u64(),
            old["build"]["generation"].as_u64(),
        ) {
            (Some(new_generation), Some(old_generation)) => {
                new["build"]["commit"] != old["build"]["commit"] && new_generation > old_generation
            }
            (Some(_), None) | (None, None) => true,
            (None, Some(_)) => false,
        },
    })
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Engine => "Ara engine",
        Kind::Hub => "Ara Hub",
    }
}

/// One question while the folder is blocked: true tries again, false cancels.
fn ask(plan: &Plan, host: &mut dyn Host, blockers: &[Blocker]) -> Result<bool> {
    match host.resolve(plan, blockers) {
        Resolution::Retry => Ok(true),
        Resolution::StopService => {
            if let Some(service) = blockers.iter().find(|blocker| blocker.service) {
                host.progress(Step::StopService);
                processes::stop_hub_service(service)?;
            }
            Ok(true)
        }
        Resolution::Cancel => Ok(false),
    }
}

enum Swapped {
    Done { leftover: Option<PathBuf> },
    Cancelled,
}

/// Previous installation → backup sibling, stage → destination, delete the backup.
/// Until the new files are in place every failure restores the previous install.
fn swap(stage: Stage, plan: &Plan, args: &Args, host: &mut dyn Host) -> Result<Swapped> {
    let destination = &plan.destination;
    let backup = sibling(destination, "replaced")?;
    while let Err(error) = rename_retrying(destination, &backup) {
        if args.silent {
            bail!(
                "ARA-EXP-INSTALL-008: could not move the previous installation aside ({error}); nothing was changed. Close programs using {} and run setup again",
                destination.display()
            );
        }
        if !ask(plan, host, &processes::running_from(destination))? {
            return Ok(Swapped::Cancelled);
        }
    }
    if let Err((stage, error)) = stage.rename_to(destination) {
        drop(stage);
        if let Err(restore) = rename_retrying(&backup, destination) {
            bail!(
                "ARA-EXP-INSTALL-009: could not move the new files into place ({error}) or restore the previous installation ({restore}). It is intact at {}; rename it back to {}",
                backup.display(),
                destination.display()
            );
        }
        bail!(
            "ARA-EXP-INSTALL-008: could not move the new files into place ({error}); the previous installation was restored"
        );
    }
    host.progress(Step::Cleanup);
    Ok(Swapped::Done {
        leftover: remove_retrying(&backup).err().map(|_| backup),
    })
}

/// A unique hidden-by-convention sibling: `.ara-<purpose>-<pid>-<nanos>`.
fn sibling(destination: &Path, purpose: &str) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .context("installation needs a parent directory")?;
    Ok(parent.join(format!(
        ".ara-{purpose}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    )))
}

/// Deletes a replaced installation, retrying while scanners release its files.
fn remove_retrying(path: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::remove_dir_all(path) {
            Err(error) if attempt < 5 && error.kind() != io::ErrorKind::NotFound => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            result => return result,
        }
    }
}

/// Splits `ARA-…-NNN: message` into its code and the first paragraph (the summary);
/// the full text, registration output included, stays available as details.
pub fn summarize(error: &anyhow::Error) -> (Option<String>, String) {
    let text = format!("{error:#}");
    let summary = text.split("\n\n").next().unwrap_or(&text).trim();
    let (head, rest) = summary.split_once(": ").unwrap_or(("", summary));
    let coded = head.starts_with("ARA-")
        && head
            .rsplit('-')
            .next()
            .is_some_and(|n| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()));
    if coded {
        (Some(head.to_owned()), rest.to_owned())
    } else {
        (None, summary.to_owned())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn destinations_are_classified_by_manifest_only() {
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name);
        assert_eq!(inspect(&path("missing"), Kind::Engine), Existing::Missing);
        fs::create_dir(path("engine")).unwrap();
        fs::write(
            path("engine/engine.json"),
            r#"{"format":"ara.engine/1","version":"0.0.1-alpha"}"#,
        )
        .unwrap();
        assert_eq!(
            inspect(&path("engine"), Kind::Engine),
            Existing::Install {
                version: "0.0.1-alpha".into()
            }
        );
        assert_eq!(inspect(&path("engine"), Kind::Hub), Existing::Foreign);
        fs::create_dir(path("hub")).unwrap();
        fs::write(
            path("hub/hub.json"),
            "\u{feff}{\"format\":\"ara.hub/1\",\"version\":\"0.0.1-alpha\"}",
        )
        .unwrap();
        assert!(matches!(
            inspect(&path("hub"), Kind::Hub),
            Existing::Install { .. }
        ));
        fs::create_dir(path("empty")).unwrap();
        assert_eq!(inspect(&path("empty"), Kind::Engine), Existing::Foreign);
        fs::create_dir(path("other")).unwrap();
        fs::write(
            path("other/hub.json"),
            r#"{"format":"other/1","version":"1"}"#,
        )
        .unwrap();
        assert_eq!(inspect(&path("other"), Kind::Hub), Existing::Foreign);
        fs::write(path("file"), "").unwrap();
        assert_eq!(inspect(&path("file"), Kind::Engine), Existing::Foreign);
    }

    #[test]
    fn coded_errors_split_into_code_summary_and_details() {
        let error = anyhow::anyhow!("ARA-EXP-INSTALL-010: registration failed\n\nline 1\nline 2");
        assert_eq!(
            summarize(&error),
            (
                Some("ARA-EXP-INSTALL-010".into()),
                "registration failed".into()
            )
        );
        let plain = anyhow::anyhow!("Access is denied. (os error 5)");
        assert_eq!(
            summarize(&plain),
            (None, "Access is denied. (os error 5)".into())
        );
    }
}
