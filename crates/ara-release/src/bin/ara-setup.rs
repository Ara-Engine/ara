//! Ara SFX stub: validated extraction followed by per-user engine or Hub setup.
use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::{path::PathBuf, process::Command};

#[derive(Parser)]
#[command(version, about = "Install Ara for this Windows user")]
struct Args {
    /// New destination (default: Programs/Ara/<version>, or Programs/Ara Hub).
    #[arg(long)]
    destination: Option<PathBuf>,
    /// No UI or completion dialog.
    #[arg(long)]
    silent: bool,
    /// Isolate shell integration for automated smoke checks.
    #[arg(long)]
    no_system_integration: bool,
    /// List validated payload names without installing.
    #[arg(long)]
    list: bool,
}
fn main() -> Result<()> {
    let args = Args::parse();
    let bytes = std::fs::read(std::env::current_exe()?)?;
    let names = ara_export_installer::list(&bytes)?;
    if args.list {
        for name in &names {
            println!("{name}");
        }
        return Ok(());
    }
    let standalone = names.iter().any(|name| name == "hub.json");
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?);
    let destination = args.destination.unwrap_or(if standalone {
        local.join("Programs/Ara Hub")
    } else {
        local.join("Programs/Ara").join(env!("CARGO_PKG_VERSION"))
    });
    ara_export_installer::install(&bytes, &destination)?;
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(destination.join("install.ps1"))
        .arg("-RegisterOnly");
    if args.silent {
        command.arg("-Silent");
    }
    if args.no_system_integration {
        command.arg("-NoSystemIntegration");
    }
    let status = command.status()?;
    ensure!(
        status.success(),
        "registration failed; extracted files remain at {} for inspection. Run install.ps1 -RegisterOnly to retry",
        destination.display()
    );
    Ok(())
}
