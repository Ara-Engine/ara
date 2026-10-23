//! Explicit native plugin lane; the template lane never compiles Rust.
use crate::{
    ExportPreset, Platform, android,
    tools::{self, Toolchain},
};
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
pub(crate) fn compile_plugins(
    p: &ExportPreset,
    project: &Path,
    t: &Toolchain,
    work: &Path,
) -> Result<Vec<(String, PathBuf)>> {
    let targets: Vec<_> = match p.platform {
        Platform::Windows => vec![("x64", "x86_64-pc-windows-msvc")],
        Platform::Linux => vec![("x64", "x86_64-unknown-linux-gnu")],
        Platform::Android => android::ABIS.iter().map(|(a, tr, _)| (*a, *tr)).collect(),
        _ => vec![],
    };
    let mut artifacts = Vec::new();
    for plugin in &p.plugins {
        for (arch, target) in &targets {
            let manifest = tools::project_path(project, &plugin.manifest)?;
            let build = work.join("native").join(&plugin.library);
            let mut cmd = Command::new("cargo");
            cmd.arg("build")
                .arg("--release")
                .arg("--manifest-path")
                .arg(manifest)
                .args(["--target", target])
                .arg("--target-dir")
                .arg(&build)
                .env("CARGO_PROFILE_RELEASE_DEBUG", "line-tables-only")
                .env("CARGO_PROFILE_RELEASE_STRIP", "none");
            let envkey = format!(
                "CARGO_TARGET_{}_LINKER",
                target.to_uppercase().replace('-', "_")
            );
            if p.platform == Platform::Linux {
                cmd.env(
                    &envkey,
                    t.zigcc.as_ref().context("ARA-EXP-LNX-007: zigcc missing")?,
                )
                .env(
                    "ARA_ZIG",
                    t.zig.as_ref().context("ARA-EXP-LNX-006: Zig missing")?,
                );
            }
            if p.platform == Platform::Android {
                cmd.env(
                    &envkey,
                    t.ndk_bin()
                        .context("NDK missing")?
                        .join(tools::executable("clang")),
                )
                .env(
                    format!(
                        "CARGO_TARGET_{}_RUSTFLAGS",
                        target.to_uppercase().replace('-', "_")
                    ),
                    format!(
                        "-C link-arg=--target={target}{} -C link-arg=-Wl,-z,max-page-size=16384",
                        p.min_api
                    ),
                );
            }
            tools::run(
                &mut cmd,
                &work
                    .join("logs")
                    .join(format!("native-{}-{arch}.log", plugin.library)),
                Duration::from_secs(300),
            )?;
            let name = if p.platform == Platform::Windows {
                format!("{}.dll", plugin.library)
            } else {
                format!("lib{}.so", plugin.library)
            };
            artifacts.push((
                arch.to_string(),
                build.join(target).join("release").join(name),
            ));
        }
    }
    Ok(artifacts)
}
