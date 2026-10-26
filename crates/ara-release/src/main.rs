//! Windows packages, signed manifests and explicit opt-in GitHub publication.
mod archive;
mod hub;
mod notices;
mod package;
mod publish;
mod references;
mod smoke;
mod support;

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Package and verify an installable Ara engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Exercise signed dry-run releases through a loopback feed and installed binaries.
    #[cfg(feature = "rehearsal")]
    Rehearse {
        #[arg(long)]
        artifacts: PathBuf,
        #[arg(long)]
        older_setup: PathBuf,
        #[arg(long)]
        directory: PathBuf,
    },
    /// Build release binaries, portable ZIP, self-extracting installer and symbols.
    Package {
        #[arg(long)]
        version: String,
        #[arg(long, value_enum, default_value = "engine")]
        kind: publish::ReleaseKind,
    },
    /// Copy the reviewed RELEASE_NOTES.md into the release artifacts directory.
    Notes {
        #[arg(long)]
        version: String,
    },
    /// Check that every file a template's documents reference exists inside it
    /// (the packaging gate), e.g. on a sample after `ara-samples kits`.
    References {
        #[arg(required = true)]
        templates: Vec<PathBuf>,
    },
    /// Install the SFX into scratch space and exercise the installed build.
    Smoke {
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
        /// Existing packaged artifacts (default target/release-dist).
        #[arg(long)]
        artifacts: Option<PathBuf>,
    },
    /// Convert a private symbol archive using ara-crash (explicit opt-in).
    Symbols { directory: PathBuf },
    /// Generate a private signing seed outside the checkout; print only its public key.
    Keygen {
        #[arg(long)]
        output: PathBuf,
    },
    /// Sign an existing release manifest.
    Sign {
        manifest: PathBuf,
        #[arg(long)]
        key: Option<PathBuf>,
    },
    /// Prepare a signed release folder and optionally upload through the developer gh CLI.
    Publish {
        /// GitHub owner/name; overrides ARA_RELEASE_REPO and the built-in default.
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        prerelease: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        key: Option<PathBuf>,
        /// Existing packaged artifacts (default target/release-dist).
        #[arg(long)]
        artifacts: Option<PathBuf>,
    },
}
fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .context("release crate must live under crates/")?
        .to_path_buf();
    match Cli::parse().command {
        #[cfg(feature = "rehearsal")]
        Command::Rehearse {
            artifacts,
            older_setup,
            directory,
        } => smoke::rehearsal::run(&artifacts, &older_setup, &directory),
        Command::Package { version, kind } => {
            valid_version(&version)?;
            match kind {
                publish::ReleaseKind::Hub => hub::package(&root, &version),
                publish::ReleaseKind::Engine => package::package(&root, &version),
            }
        }
        Command::Notes { version } => {
            valid_version(&version)?;
            package::notes(&root, &version)
        }
        Command::References { templates } => {
            for template in &templates {
                references::check(template)?;
                println!("{}: every reference resolves", template.display());
            }
            Ok(())
        }
        Command::Smoke { version, artifacts } => {
            valid_version(&version)?;
            smoke::smoke(&artifacts.unwrap_or_else(|| support::dist(&root)), &version)
        }
        Command::Symbols { directory } => package::symbols(&root, &directory),
        Command::Keygen { output } => publish::keygen(&root, &output),
        Command::Sign { manifest, key } => publish::sign_file(&manifest, key.as_deref()),
        Command::Publish {
            repo,
            tag,
            prerelease,
            dry_run,
            key,
            artifacts,
        } => publish::publish(
            &root,
            &release_repo(repo, std::env::var("ARA_RELEASE_REPO").ok()),
            &tag,
            prerelease,
            dry_run,
            key.as_deref(),
            artifacts.as_deref(),
        ),
    }
}
fn release_repo(explicit: Option<String>, environment: Option<String>) -> String {
    explicit
        .or(environment)
        .unwrap_or_else(|| ara_update::DEFAULT_REPOSITORY.to_owned())
}
fn valid_version(version: &str) -> Result<()> {
    ensure!(
        version == env!("CARGO_PKG_VERSION"),
        "version must match workspace.package.version ({})",
        env!("CARGO_PKG_VERSION")
    );
    ensure!(
        version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c)),
        "invalid version path"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn publication_repo_preserves_explicit_and_environment_overrides() {
        assert_eq!(super::release_repo(None, None), "Ara-Engine/ara");
        assert_eq!(
            super::release_repo(None, Some("custom/releases".into())),
            "custom/releases"
        );
        assert_eq!(
            super::release_repo(
                Some("explicit/releases".into()),
                Some("custom/releases".into())
            ),
            "explicit/releases"
        );
    }
    #[test]
    fn version_cannot_escape_output_or_mislabel_binaries() {
        assert!(super::valid_version(env!("CARGO_PKG_VERSION")).is_ok());
        for version in ["../escape", "1.2.3", "0.0.1-alpha/other"] {
            assert!(super::valid_version(version).is_err());
        }
    }
}
