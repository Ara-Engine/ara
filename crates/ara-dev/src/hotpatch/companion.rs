//! Editor Play uses a freshly paired player, never an executable from an older build.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) fn prepare(root: &Path, artifacts: &Path) -> Result<PathBuf> {
    let directory = artifacts.join("companion");
    fs::create_dir_all(&directory)?;
    let log = directory.join("cargo.jsonl");
    let mut build = super::tool::command("cargo");
    build
        .current_dir(root)
        .args([
            "build",
            "--locked",
            "--profile",
            "hotpatch",
            "--message-format=json-render-diagnostics",
            "-p",
            "ara-editor-hotpatch",
            "-p",
            "ara-player",
            "--bins",
            "--features",
            "ara-editor-hotpatch/hotpatch",
        ])
        .env("CARGO_TARGET_DIR", directory.join("build"))
        .env("CARGO_BUILD_BUILD_DIR", directory.join("build"))
        .env(
            "CARGO_BUILD_JOBS",
            if cfg!(target_os = "macos") { "2" } else { "4" },
        )
        .env("RUSTC_WRAPPER", "")
        .stdout(fs::File::create(&log)?);
    eprintln!("Building editor + matching Play companion before the initial dx build...");
    ensure!(
        crate::process::run(&mut build, None)?.success(),
        "editor/player companion build failed; see {}",
        log.display()
    );
    let binaries = crate::binaries::artifacts(&fs::read_to_string(&log)?)?;
    let player = binaries
        .get("ara-player")
        .context("missing matching Play companion")?;
    #[cfg(windows)]
    super::linker::prepare_companion(player)?;
    if let Some(existing) = std::env::var_os("ARA_PLAYER").filter(|p| !p.is_empty()) {
        ensure!(
            root.join(existing).canonicalize().ok() == Some(player.canonicalize()?),
            "ARA7600: stale ARA_PLAYER override; unset it and rerun ara-dev hotpatch"
        );
    }
    for rt in [false, true] {
        let output = directory.join(format!("ray-tracing-{rt}.log"));
        let mut probe = super::tool::command(player);
        probe
            .current_dir(root)
            .args([format!("--ray-tracing={rt}"), "--help".into()])
            .env("ARA_CONFIG_DIR", directory.join("probe-config"))
            .env("ARA_CRASH_DIR", directory.join("probe-crashes"))
            .stdout(fs::File::create(&output)?)
            .stderr(fs::File::create(output.with_extension("err"))?);
        ensure!(
            crate::process::run(&mut probe, Some(Instant::now() + Duration::from_secs(15)))?
                .success()
                && fs::read_to_string(&output)?.contains("--ray-tracing"),
            "ARA7600: matching Play companion failed --ray-tracing={rt} verification"
        );
    }
    eprintln!(
        "Matching Play companion verified. Its code stays fixed during editor UI patching; use the player hotpatch lane for player Rust iteration."
    );
    Ok(player.to_owned())
}
