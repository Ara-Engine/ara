//! Explicit optional-tool setup; never reached by normal Cargo builds or tests.
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const REVISION: &str = "cb27832398e8a3fa30a0fc8d6917c6d8a86fa71a";

pub(super) fn binary(root: &Path) -> PathBuf {
    super::output_root(root)
        .join("ara-hotpatch/dx/bin")
        .join(if cfg!(windows) { "dx.exe" } else { "dx" })
}

pub(super) fn select(root: &Path, explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os("ARA_DX").map(PathBuf::from))
        .unwrap_or_else(|| {
            let local = binary(root);
            if local.is_file() { local } else { "dx".into() }
        })
}

pub(super) fn command(executable: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(executable);
    command.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW,
        };
        command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
    }
    command
}

pub(super) fn capability(dx: &Path) -> Result<()> {
    let output = command(dx)
        .arg("--ara-native-rust-patch-version")
        .output()?;
    ensure!(
        output.status.success() && output.stdout.trim_ascii() == b"2",
        "ARA7600: dx lacks Ara's native Rust save fix; run ara-dev hotpatch --install-dx, or set ARA_DX to the prepared tool"
    );
    Ok(())
}

fn run(command: &mut Command) -> Result<()> {
    ensure!(
        crate::process::run(command, None)?.success(),
        "ARA7600: optional dx setup command failed"
    );
    Ok(())
}

pub(super) fn install(root: &Path, plan: bool) -> Result<bool> {
    if plan {
        println!(
            "optional dx {}; upstream={REVISION}; native Rust save/recovery fix v2; output={}",
            super::DX_VERSION,
            binary(root).display()
        );
        return Ok(true);
    }
    let installed = binary(root);
    if installed.is_file() && super::available(&installed).is_ok() {
        println!("Optional native dx already ready: {}", installed.display());
        return Ok(true);
    }
    let directory = super::output_root(root).join("ara-hotpatch/dx");
    let source = directory.join("source");
    std::fs::create_dir_all(&directory)?;
    if !source.exists() {
        eprintln!("Preparing optional dx source (normal Ara builds never need this tool)...");
        run(command("git")
            .current_dir(root)
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                "v0.8.0-alpha.3",
                "https://github.com/DioxusLabs/dioxus.git",
            ])
            .arg(&source))?;
    }
    let revision = command("git")
        .arg("-C")
        .arg(&source)
        .args(["rev-parse", "HEAD"])
        .output()?;
    ensure!(
        revision.status.success() && revision.stdout.trim_ascii() == REVISION.as_bytes(),
        "ARA7600: cached optional dx source has an unexpected revision"
    );
    let patch = directory.join("native-watch.patch");
    std::fs::write(&patch, include_bytes!("dx-native-watch.patch"))?;
    let patched = command("git")
        .arg("-C")
        .arg(&source)
        .args(["apply", "--unidiff-zero", "--reverse", "--check"])
        .arg(&patch)
        .output()?;
    if !patched.status.success() {
        run(command("git")
            .arg("-C")
            .arg(&source)
            .arg("apply")
            .arg("--unidiff-zero")
            .arg(&patch))?;
    }
    let diff = command("git")
        .arg("-C")
        .arg(&source)
        .args([
            "diff",
            "--binary",
            "--abbrev=7",
            "--unified=0",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ])
        .output()?;
    ensure!(
        diff.status.success()
            && diff.stdout.trim_ascii() == include_bytes!("dx-native-watch.patch").trim_ascii(),
        "ARA7600: cached dx source has changes beyond the pinned native Rust patch"
    );
    let channel = std::fs::read_to_string(root.join("rust-toolchain.toml"))?
        .parse::<toml_edit::DocumentMut>()?;
    let channel = channel["toolchain"]["channel"]
        .as_str()
        .context("pinned Rust channel is missing")?;
    let build_dir = directory.join("build");
    #[cfg(target_os = "macos")]
    let mut build = {
        let mut c = command("caffeinate");
        c.args(["-dimsu", "nice", "-n", "10", "cargo"]);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut build = command("cargo");
    build
        .current_dir(&source)
        .args([
            "build",
            "--locked",
            "--release",
            "-p",
            "dioxus-cli",
            "--bin",
            "dx",
        ])
        .env("RUSTUP_TOOLCHAIN", channel)
        .env(
            "CARGO_BUILD_JOBS",
            if cfg!(target_os = "macos") { "2" } else { "4" },
        )
        .env("CARGO_TARGET_DIR", &build_dir)
        .env("CARGO_BUILD_BUILD_DIR", &build_dir)
        .env("CARGO_PROFILE_RELEASE_DEBUG", "0")
        .env("CARGO_PROFILE_RELEASE_LTO", "false")
        // Default release partitioning avoids hundreds of temporary object files
        // per dependency on slower build disks. This does not affect engine patches.
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16")
        .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "2")
        .env("RUSTC_WRAPPER", "");
    eprintln!(
        "Building optional native dx once; logs remain attached. This can take several minutes."
    );
    run(&mut build)?;
    let destination = binary(root);
    let built = build_dir
        .join("release")
        .join(destination.file_name().context("dx filename")?);
    super::available(&built)?;
    std::fs::create_dir_all(destination.parent().context("dx output directory")?)?;
    let temporary = destination.with_extension("installing");
    std::fs::copy(&built, &temporary)?;
    std::fs::rename(&temporary, &destination).context(
        "install optional dx; close running hotpatch sessions if this executable is in use",
    )?;
    println!("Optional native dx ready: {}", destination.display());
    Ok(true)
}
