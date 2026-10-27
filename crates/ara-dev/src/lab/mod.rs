//! One checkout-owned iteration daemon. Clients never invoke Cargo or own a player.
mod ab;
pub(crate) mod build;
mod claims;
mod daemon;
mod generations;
mod protocol;
mod references;
mod scenes;
mod session;
mod track;
mod variants;
mod watch;
pub(crate) use daemon::run_with_strategy;

use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Args)]
pub(crate) struct Options {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Start the singleton; returns once its endpoint is reachable.
    Start {
        /// Run attached to this terminal (the default detaches with hidden children).
        #[arg(long)]
        foreground: bool,
        /// Shared Cargo outputs; overrides ARA_LAB_BUILD_DIR and the saved workspace choice.
        #[arg(long)]
        build_dir: Option<PathBuf>,
        /// Maximum resident track players; admission also checks available VRAM.
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=8))]
        max_sessions: u32,
    },
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        build_dir: PathBuf,
        #[arg(long)]
        max_sessions: u32,
    },
    /// Serve lab tools over newline-delimited stdio MCP; stdout is protocol only.
    Mcp,
    /// Read daemon, build, shader, claim and warm-player state.
    Status,
    /// Declare ownership; overlapping claims are returned as warnings.
    Claim {
        #[arg(long)]
        agent: String,
        #[arg(required = true)]
        paths: Vec<String>,
    },
    Release {
        #[arg(long)]
        agent: String,
    },
    /// Reload successfully cooked shaders in the selected track.
    Reload {
        #[arg(long, default_value = "default")]
        session: String,
    },
    /// Queue one incremental build; the old players survive compiler failures.
    Rebuild,
    /// Open or select a hidden track on a frozen scene.
    Session {
        #[arg(long, default_value = "default")]
        session: String,
        /// Retire this track and free its resident-player slot.
        #[arg(long)]
        close: bool,
        #[arg(long, default_value = "showcase", value_parser = ["showcase", "outpost", "hangar"])]
        scene: String,
        #[arg(long)]
        pack: Option<PathBuf>,
        #[arg(long, default_value = "high", value_parser = ["low", "medium", "high", "epic"])]
        preset: String,
        #[arg(long, default_value_t = 2560)]
        width: u32,
        #[arg(long, default_value_t = 1440)]
        height: u32,
    },
    /// Capture fixed cameras in the existing track, with reference metrics and crops.
    Capture {
        #[arg(long, default_value = "default")]
        session: String,
        /// One sweep without temporal/repeatability gates for the shortest visual loop.
        #[arg(long)]
        quick: bool,
    },
    /// Exclusive, guarded steady-state frame window.
    Measure {
        #[arg(long, default_value = "default")]
        session: String,
        #[arg(long, default_value_t = 120)]
        frames: u32,
        #[arg(long, default_value = "high")]
        preset: String,
        #[arg(long)]
        no_wait: bool,
        #[command(flatten)]
        guard: crate::gpu_guard::Policy,
    },
    /// Register/select a compile-time shader variant; baseline is always available.
    Variant {
        #[arg(long, default_value = "default")]
        session: String,
        #[arg(long)]
        name: String,
        /// JSON {packages:{"shaders/pipelines.json":{pipelines:{...}}}}.
        #[arg(long)]
        recipe: Option<PathBuf>,
        /// Promote a successfully cooked recipe into baseline manifests.
        #[arg(long)]
        promote: bool,
        /// Forget an unused recipe; retain its cached artifacts for inspection.
        #[arg(long,conflicts_with_all=["recipe","promote"])]
        remove: bool,
    },
    /// Interleave baseline and experiment windows in the same warm player.
    Ab {
        #[arg(long, default_value = "default")]
        session: String,
        #[arg(long)]
        variant: String,
        #[arg(long, default_value_t = 4)]
        windows: u32,
        #[arg(long, default_value_t = 120)]
        frames: u32,
        #[arg(long, default_value = "high")]
        preset: String,
        #[arg(long)]
        no_wait: bool,
        #[command(flatten)]
        guard: crate::gpu_guard::Policy,
    },
    /// Discover or change typed live tunables (JSON name -> {type,value}).
    Set {
        #[arg(long, default_value = "default")]
        session: String,
        #[arg(long)]
        values: Option<String>,
        /// Select one of the scene's fixed cameras without restarting the track.
        #[arg(long)]
        camera: Option<String>,
    },
    /// Render optional Blender 5.1 scene-linear EXR references under the lab cache.
    Reference {
        #[arg(long, default_value = "showcase")]
        scene: String,
        #[arg(long)]
        blender: Option<PathBuf>,
        #[arg(long)]
        bench_root: Option<PathBuf>,
        #[arg(long, default_value_t = 128)]
        samples: u32,
    },
    Logs {
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    /// Stop only this daemon's exact children and release its checkout lease.
    Stop,
}

pub(super) fn directory(root: &Path) -> PathBuf {
    root.join("target/ara-dev/lab")
}

pub(crate) fn run(root: &Path, options: Options) -> Result<bool> {
    let (tool, arguments) = match options.action {
        Action::Start {
            foreground,
            build_dir,
            max_sessions,
        } => {
            let build_dir = build_dir.unwrap_or_else(|| build::default_directory(root));
            if foreground {
                return daemon::run(root, &build_dir, max_sessions);
            }
            if let Ok(status) = protocol::call(root, "lab_status", json!({})) {
                println!("{status}");
                return Ok(true);
            }
            let base = directory(root);
            fs::create_dir_all(&base)?;
            let source = std::env::current_exe()?;
            let binary_dir = base.join("bin").join(session::id());
            fs::create_dir_all(&binary_dir)?;
            let executable = binary_dir.join(
                source
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("lab executable filename"))?,
            );
            fs::copy(source, &executable)?;
            let mut command = Command::new(executable);
            command
                .current_dir(root)
                .arg("lab")
                .arg("serve")
                .arg("--build-dir")
                .arg(build_dir)
                .arg("--max-sessions")
                .arg(max_sessions.to_string())
                .stdin(Stdio::null())
                .stdout(fs::File::create(base.join("daemon.log"))?)
                .stderr(fs::File::create(base.join("daemon.stderr.log"))?);
            build::daemon(&mut command)?;
            let mut child = command.spawn()?;
            let started = std::time::Instant::now();
            loop {
                if let Ok(value) = protocol::call(root, "lab_status", json!({})) {
                    println!("{value}");
                    return Ok(true);
                }
                ensure!(
                    child.try_wait()?.is_none(),
                    "lab startup failed; inspect {}",
                    base.join("daemon.stderr.log").display()
                );
                ensure!(
                    started.elapsed().as_secs() < 15,
                    "lab endpoint did not appear; inspect daemon logs"
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        Action::Serve {
            build_dir,
            max_sessions,
        } => return daemon::run(root, &build_dir, max_sessions),
        Action::Mcp => return protocol::mcp(root),
        Action::Status => ("lab_status", json!({})),
        Action::Claim { agent, paths } => ("lab_claim", json!({"agent":agent,"paths":paths})),
        Action::Release { agent } => ("lab_release", json!({"agent":agent})),
        Action::Reload { session } => ("lab_reload", json!({"session":session})),
        Action::Rebuild => ("lab_rebuild", json!({})),
        Action::Session {
            session,
            close,
            scene,
            pack,
            preset,
            width,
            height,
        } => (
            "lab_session",
            json!({"session":session,"close":close,"scene":scene,"pack":pack,"preset":preset,"width":width,"height":height}),
        ),
        Action::Capture { session, quick } => (
            "lab_capture",
            json!({"session":session,"runs":if quick{1}else{3},"temporal":!quick}),
        ),
        Action::Measure {
            session,
            frames,
            preset,
            no_wait,
            guard,
        } => (
            "lab_measure",
            json!({"session":session,"frames":frames,"preset":preset,"no_wait":no_wait,"guard":guard}),
        ),
        Action::Variant {
            session,
            name,
            recipe,
            promote,
            remove,
        } => {
            let recipe = recipe
                .map(|path| -> Result<Value> { Ok(serde_json::from_slice(&fs::read(path)?)?) })
                .transpose()?;
            (
                "lab_variant",
                json!({"session":session,"name":name,"recipe":recipe,"promote":promote,"remove":remove}),
            )
        }
        Action::Ab {
            session,
            variant,
            windows,
            frames,
            preset,
            no_wait,
            guard,
        } => (
            "lab_ab",
            json!({"session":session,"b":variant,"windows":windows,"frames":frames,"preset":preset,"no_wait":no_wait,"guard":guard}),
        ),
        Action::Set {
            session,
            values,
            camera,
        } => (
            "lab_set",
            json!({"session":session,"camera":camera,"values":values.map(|s|serde_json::from_str::<Value>(&s)).transpose()?.unwrap_or_else(||json!({}))}),
        ),
        Action::Reference {
            scene,
            blender,
            bench_root,
            samples,
        } => (
            "lab_reference",
            json!({"scene":scene,"blender":blender,"bench_root":bench_root,"samples":samples}),
        ),
        Action::Logs { lines } => ("lab_logs", json!({"lines":lines})),
        Action::Stop => ("lab_stop", json!({})),
    };
    let value: Value = protocol::call(root, tool, arguments)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(true)
}
