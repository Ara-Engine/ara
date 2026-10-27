//! Optional dx lane. The lab can probe availability before selecting this strategy.
mod compiler;
pub(crate) mod linker;
mod response;
#[cfg(windows)]
mod windows_stubs;
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

// alpha.3 fixes workspace replay of crates absent from the selected host. Its
// subsecond-types wire schema is identical to 0.7.10, used by Bevy's HotFn table.
const DX_VERSION: &str = "0.8.0-alpha.3";

pub(crate) fn forward() -> Option<std::process::ExitCode> {
    linker::forward().or_else(compiler::forward)
}

#[derive(Args)]
pub(crate) struct Options {
    /// Explicit dx executable (otherwise ARA_DX, then PATH).
    #[arg(long)]
    dx: Option<PathBuf>,
    /// Local devserver port, distinct for each simultaneous host.
    #[arg(long, default_value_t = 18080)]
    port: u16,
    /// Print the selected target/tool version without compiling or launching.
    #[arg(long)]
    plan: bool,
    /// player|editor followed by native application arguments.
    #[arg(last = true, required = true, num_args = 1..)]
    args: Vec<OsString>,
}

/// Cheap lab capability probe. An error selects its existing warm-restart strategy.
pub(crate) fn available(dx: &Path) -> Result<()> {
    let output = Command::new(dx).arg("--version").output()
        .with_context(|| format!("ARA7600: optional dx is missing at {}; install dioxus-cli {DX_VERSION} or set ARA_DX", dx.display()))?;
    ensure!(output.status.success(), "ARA7600: dx --version failed");
    let text = String::from_utf8(output.stdout)?;
    ensure!(
        text.split_whitespace().nth(1) == Some(DX_VERSION),
        "ARA7600: expected dioxus-cli {DX_VERSION}, got {}; set ARA_DX to a matching tool",
        text.trim()
    );
    Ok(())
}

fn target(args: &[OsString]) -> Result<(&'static str, &'static str, &'static str)> {
    match args.first().and_then(|a| a.to_str()) {
        Some("player" | "ara-player") => Ok(("ara-player", "ara-player", "hotpatch")),
        Some("editor" | "ara-editor") => {
            Ok(("ara-editor-hotpatch", "ara-editor-hotpatch", "hotpatch"))
        }
        Some("render-probe") => Ok(("ara-hotpatch-probe", "ara-hotpatch-probe", "hotpatch")),
        _ => bail!("use ara-dev hotpatch -- player|editor [native arguments]"),
    }
}

/// Launch the native host; all runtime diagnostics retain the host PID and generation.
pub(crate) fn run(root: &Path, options: Options) -> Result<bool> {
    let (package, bin, feature) = target(&options.args)?;
    let dx = options
        .dx
        .or_else(|| std::env::var_os("ARA_DX").map(PathBuf::from))
        .unwrap_or_else(|| "dx".into());
    available(&dx)?;
    if options.plan {
        println!(
            "dx={}; version={DX_VERSION}; package={package}; bin={bin}; feature={feature}; profile=hotpatch; port={}",
            dx.display(),
            options.port
        );
        return Ok(true);
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let directory = root
        .join("target/ara-hotpatch")
        .join(format!("run-{stamp}"));
    std::fs::create_dir_all(&directory)?;
    let contract_path = directory.join("contract.json");
    eprintln!("Capturing Rust compatibility baseline (off the frame thread)...");
    ara_hotpatch::Contract::capture(root)?.write(&contract_path)?;
    let mut command = Command::new(&dx);
    command.current_dir(root).args([
        "serve",
        "--platform",
        "desktop",
        "--hot-patch",
        "--interactive",
        "false",
        "--open",
        "false",
        "--always-on-top",
        "false",
        "--skip-assets",
        "--addr",
        "127.0.0.1",
        "--profile",
        "hotpatch",
        "--package",
        package,
        "--bin",
        bin,
        "--features",
        feature,
        "--port",
        &options.port.to_string(),
    ]);
    let cache = directory.join("dx");
    std::fs::create_dir_all(&cache)?;
    command
        .arg("--verbose")
        .arg("--session-cache-dir")
        .arg(cache);
    // DX 0.7.10 splits --args on whitespace. Native hosts consume this envelope
    // before starting threads, so user paths/quotes never become shell syntax.
    command
        .env("ARA_HOTPATCH_ARGS", native_args(&options.args[1..])?)
        .env("ARA_HOTPATCH_DRIVER", std::env::current_exe()?);
    command
        .env("ARA_HOTPATCH_CONTRACT", contract_path)
        .env(
            "CARGO_BUILD_JOBS",
            if cfg!(target_os = "macos") { "2" } else { "4" },
        )
        .env("CARGO_TARGET_DIR", root.join("target/ara-hotpatch/build"))
        .env(
            "CARGO_BUILD_BUILD_DIR",
            root.join("target/ara-hotpatch/intermediates"),
        )
        .env("RUSTC_WRAPPER", "")
        .env("ARA_CONFIG_DIR", directory.join("profile"))
        .env("ARA_CRASH_DIR", directory.join("crashes"));
    #[cfg(target_os = "macos")]
    command.env("SDL_MAC_BACKGROUND_APP", "1");
    linker::configure(root, &mut command)?;
    eprintln!(
        "Rust hotpatch: initial fat build, then body-only patches; rebuild on ARA7602. dx remains optional."
    );
    Ok(crate::process::run(&mut command, None)?.success())
}

// Keep arguments as data; dx's whitespace splitting cannot preserve native argv.
fn native_args(args: &[OsString]) -> Result<String> {
    let strings = args
        .iter()
        .map(|arg| arg.to_str().context("dx native arguments must be UTF-8"))
        .collect::<Result<Vec<_>>>()?;
    serde_json::to_string(&strings).context("encode native hotpatch arguments")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_uses_a_bin_only_tip_and_arguments_remain_data() {
        assert_eq!(
            target(&["editor".into()]).unwrap(),
            ("ara-editor-hotpatch", "ara-editor-hotpatch", "hotpatch")
        );
        let encoded = native_args(&["C:\\a b".into(), "it's".into(), "$(literal)".into()]).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&encoded).unwrap(),
            ["C:\\a b", "it's", "$(literal)"]
        );
        assert!(target(&["release".into()]).is_err());
    }
}
