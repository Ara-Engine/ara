//! Optimized iteration builds and serialized measurements of immutable exported scenes.
mod control;
mod freeze;
mod report;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand, ValueEnum};

use crate::{binaries::Binaries, build_paths::Directories, process};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Preset {
    Low,
    Medium,
    High,
    Epic,
}

impl Preset {
    fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Epic => "epic",
        }
    }
}

#[derive(Args)]
pub(crate) struct Options {
    /// Frozen exported folder with ara-launch.json; the default Outpost export is cached once.
    #[arg(long)]
    scene: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "high")]
    preset: Preset,
    #[arg(long, default_value_t = 240, value_parser = clap::value_parser!(u32).range(1..))]
    frames: u32,
    #[arg(long, default_value_t = 30)]
    warmup: u32,
    #[arg(long, default_value_t = 2560, value_parser = clap::value_parser!(u32).range(1..))]
    width: u32,
    #[arg(long, default_value_t = 1440, value_parser = clap::value_parser!(u32).range(1..))]
    height: u32,
    /// Keep the player and GPU lock alive; use `perf control` from another shell.
    #[arg(long)]
    keep_alive: bool,
    /// Reuse an existing build; useful for shader-only iterations.
    #[arg(long)]
    no_build: bool,
    #[arg(long)]
    player: Option<PathBuf>,
    /// Optional presented capture after the measurement (readback excluded from frame timings).
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Defaults to perf; release is available for D26 confirmation and comparisons.
    #[arg(long, default_value = "perf", value_parser = ["perf", "release"])]
    profile: String,
    /// Compare with this report instead of the previous compatible run.
    #[arg(long)]
    baseline: Option<PathBuf>,
    #[arg(long)]
    json: bool,
    #[arg(long, default_value_t = 3600)]
    timeout: u64,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Serve live status/measurement/settings/reload/stop tools over stdio MCP.
    Mcp {
        #[arg(long)]
        session: PathBuf,
    },
    /// Ask an existing warm player to measure, reload pipelines or change quality.
    Control {
        #[arg(long)]
        session: PathBuf,
        #[arg(long, value_enum)]
        preset: Option<Preset>,
        #[arg(long)]
        reload_shaders: bool,
        #[arg(long)]
        stop: bool,
    },
}

fn run_command(root: &Path, command: &mut Command, label: &str, timeout: u64) -> Result<()> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(timeout))
        .context("timeout overflow")?;
    ensure!(
        process::run(command.current_dir(root), Some(deadline))?.success(),
        "{label} failed; inspect its log"
    );
    Ok(())
}

fn build(root: &Path, directory: &Path, profile: &str) -> Result<(Binaries, f64)> {
    let started = Instant::now();
    let directories = Directories::from_env(root)?;
    let _lease = crate::compile_cache::acquire(started.checked_add(Duration::from_secs(3600)))?;
    let mut command = Command::new("cargo");
    directories.apply(&mut command);
    command.args([
        "build",
        "--locked",
        "--profile",
        profile,
        "--message-format=json",
    ]);
    // Always keep the same roots, even after the one-time export, to retain the feature union.
    for package in [
        "ara-player",
        "ara-shader-cook",
        "ara-export",
        "ara-import-worker",
    ] {
        command.args(["-p", package]);
    }
    command
        .env(
            "CARGO_BUILD_JOBS",
            std::env::var("CARGO_BUILD_JOBS")
                .ok()
                .and_then(|jobs| jobs.parse::<u32>().ok())
                .unwrap_or(2)
                .clamp(1, 4)
                .to_string(),
        )
        .env("RUSTC_WRAPPER", "")
        .stdout(File::create(directory.join("build.jsonl"))?)
        .stderr(File::create(directory.join("build.log"))?);
    eprintln!(
        "Building {profile}; {}",
        directory.join("build.log").display()
    );
    run_command(root, &mut command, "perf build", 3600)?;
    Ok((
        crate::binaries::artifacts(&fs::read_to_string(directory.join("build.jsonl"))?)?,
        started.elapsed().as_secs_f64(),
    ))
}

pub(crate) fn run(root: &Path, mut options: Options) -> Result<bool> {
    if let Some(action) = options.action.take() {
        return match action {
            Action::Mcp { session } => control::mcp(&session),
            Action::Control {
                session,
                preset,
                reload_shaders,
                stop,
            } => {
                let request = serde_json::json!({"quality": preset.map(Preset::name), "reload_shaders": reload_shaders, "stop": stop, "measure": !stop});
                control::enqueue(&session, &request)?;
                println!("Queued control request in {}", session.display());
                Ok(true)
            }
        };
    }
    let started = Instant::now();
    let base = root.join("target/ara-dev/perf");
    if options.scene.is_none() && !base.join("outpost/ara-launch.json").is_file() {
        freeze::preflight(root)?;
    }
    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let directory = base.join(format!("run-{id}-{}", std::process::id()));
    fs::create_dir_all(&directory)?;
    let (mut binaries, build_seconds) = if options.no_build {
        (Binaries::new(), 0.)
    } else {
        build(root, &directory, &options.profile)?
    };
    let paths_cache = base.join(format!("build-paths-{}.json", options.profile));
    if let Some(player) = &options.player {
        binaries.insert("ara-player".into(), player.canonicalize()?);
    }
    if options.no_build && !binaries.contains_key("ara-player") {
        let previous: serde_json::Value = serde_json::from_slice(
            &fs::read(&paths_cache)
                .context("--no-build needs a previous perf build or --player")?,
        )?;
        for (name, path) in previous.as_object().context("binary paths object")? {
            binaries.insert(
                name.clone(),
                PathBuf::from(path.as_str().context("binary path string")?),
            );
        }
    }
    if !options.no_build {
        fs::write(&paths_cache, serde_json::to_vec_pretty(&binaries)?)?;
    }
    let scene = if let Some(scene) = &options.scene {
        scene.canonicalize()?
    } else {
        freeze::outpost(root, &base, &binaries)?
    };
    let player = binaries
        .get("ara-player")
        .context("Cargo did not produce ara-player")?;
    ensure!(
        player.is_file(),
        "player binary is missing: {}",
        player.display()
    );
    let raw = directory.join("player.json");
    let control = directory.join("control");
    fs::create_dir_all(&control)?;
    let empty_layers = directory.join("empty-vulkan-layers");
    fs::create_dir_all(&empty_layers)?;
    eprintln!("Perf session: {}", directory.display());
    eprintln!(
        "Control: ara-dev perf control --session {}",
        control.display()
    );
    let mut command = Command::new(player);
    Directories::from_env(root)?.apply(&mut command);
    command
        .args(["--root"])
        .arg(&scene)
        .args([
            "--size",
            &format!("{}x{}", options.width, options.height),
            "--quality",
            options.preset.name(),
            "--perf-frames",
            &options.frames.to_string(),
            "--perf-warmup",
            &options.warmup.to_string(),
            "--perf-output",
        ])
        .arg(&raw)
        .arg("--perf-control")
        .arg(&control)
        .arg("--gpu-cache")
        .arg(base.join("gpu-cache"))
        .env(
            "ARA_SHADER_DIR",
            std::env::var_os("ARA_SHADER_DIR").unwrap_or_else(|| root.as_os_str().to_owned()),
        )
        .env("VK_INSTANCE_LAYERS", "")
        .env("VK_IMPLICIT_LAYER_PATH", &empty_layers)
        .env("ARA_CONFIG_DIR", directory.join("profile"))
        .env("ARA_CRASH_DIR", directory.join("crashes"))
        .stdout(File::create(directory.join("player.log"))?)
        .stderr(File::create(directory.join("player.stderr.log"))?);
    if options.keep_alive {
        command.arg("--keep-alive");
    }
    if let Some(capture) = &options.capture {
        command.arg("--capture").arg(capture);
    }
    let mut published = Vec::new();
    let mut next_report = Instant::now();
    let report = report::Destination {
        root,
        base: &base,
        directory: &directory,
        scene: &scene,
        options: &options,
        executable: player,
    };
    crate::gpu::locked(options.timeout, || {
        let deadline = if options.keep_alive {
            None
        } else {
            Some(Instant::now() + Duration::from_secs(300))
        };
        let outcome = process::run_observed(command.current_dir(root), deadline, || {
            if options.keep_alive && Instant::now() >= next_report {
                next_report = Instant::now() + Duration::from_millis(250);
                if let Ok(bytes) = fs::read(&raw)
                    && bytes != published
                {
                    let player = serde_json::from_slice(&bytes)?;
                    report.publish(player, build_seconds, started.elapsed().as_secs_f64())?;
                    published = bytes;
                }
            }
            Ok(())
        })?;
        ensure!(
            outcome.success(),
            "perf player failed or timed out; {}",
            directory.join("player.stderr.log").display()
        );
        Ok(())
    })?;
    let bytes = fs::read(&raw).context("player did not publish perf metrics")?;
    if bytes != published {
        let player: serde_json::Value = serde_json::from_slice(&bytes)?;
        report.publish(player, build_seconds, started.elapsed().as_secs_f64())?;
    }
    Ok(true)
}
