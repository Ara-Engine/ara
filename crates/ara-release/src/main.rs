//! Explicit release engineering commands; never publishes or tags.
mod archive;
mod notices;
mod package;
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
    /// Build release binaries, portable ZIP, self-extracting installer and symbols.
    Package {
        #[arg(long)]
        version: String,
    },
    /// Generate release notes from decisions, git history and known issues.
    Notes {
        #[arg(long)]
        version: String,
    },
    /// Install the SFX into scratch space and exercise the installed build.
    Smoke {
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    /// Convert a private symbol archive using ara-crash (explicit opt-in).
    Symbols { directory: PathBuf },
}
fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .context("release crate must live under crates/")?
        .to_path_buf();
    match Cli::parse().command {
        Command::Package { version } => {
            valid_version(&version)?;
            package::package(&root, &version)
        }
        Command::Notes { version } => {
            valid_version(&version)?;
            package::notes(&root, &version)
        }
        Command::Smoke { version } => {
            valid_version(&version)?;
            smoke::smoke(&root, &version)
        }
        Command::Symbols { directory } => package::symbols(&root, &directory),
    }
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
    fn version_cannot_escape_output_or_mislabel_binaries() {
        assert!(super::valid_version(env!("CARGO_PKG_VERSION")).is_ok());
        for version in ["../escape", "1.2.3", "0.0.1-alpha/other"] {
            assert!(super::valid_version(version).is_err());
        }
    }
}
