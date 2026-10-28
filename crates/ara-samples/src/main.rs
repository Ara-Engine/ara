//! `ara-samples`: the real-art sample projects under `samples/`.
//!
//! - `add-polyhaven` / `add-urls` pin a third-party asset in `samples/assets.lock.json`;
//! - `fetch` downloads (resumable, BLAKE3-verified) into each project's ignored
//!   `third_party/`; `verify` checks without downloading;
//! - `credits` regenerates `samples/THIRD_PARTY_ASSETS.md` from the lock;
//! - `build` authors a project's levels through the `ara` CLI (see `build`).
//! - `kits` writes only the generated, git-ignored kit files (release packaging).
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod build;
mod capture;
mod credits;
mod editor;
mod fetch;
mod game;
mod kit;
mod lock;
mod play;
mod sources;

/// Resolves an authored reference path to disk through the shared layout contract.
fn authored_path(root: &std::path::Path, path: impl AsRef<std::path::Path>) -> PathBuf {
    root.join(ara_doc::AUTHORING_ROOT).join(path)
}

/// Sample capture output is derived data; checked-in evidence needs an explicit opt-in.
fn evidence_directory(project: &str) -> PathBuf {
    std::env::var_os("ARA_CAPTURE_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-samples/evidence")
        })
        .join(project)
}

#[derive(Parser)]
#[command(about = "Ara sample projects with real CC0 art")]
struct Cli {
    /// The samples directory (default: `samples/` of this checkout).
    #[arg(long, global = true)]
    samples: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Author Outpost's C# co-op game layer over its existing real-art scene.
    Game,
    /// Pin a Poly Haven model, texture set or HDRI.
    AddPolyhaven {
        /// Poly Haven ID (`Barrel_01`).
        polyhaven: String,
        /// Folder ID in the lock (default: the Poly Haven ID, lowercased).
        #[arg(long)]
        id: Option<String>,
        /// `model`, `texture`, `hdri` or `character`.
        #[arg(long, default_value = "model")]
        kind: String,
        /// Resolution (`1k`, `2k`, `4k`).
        #[arg(long, default_value = "1k")]
        resolution: String,
        /// Projects that use it (comma-separated).
        #[arg(long, value_delimiter = ',', required = true)]
        projects: Vec<String>,
    },
    /// Pin files from explicit URLs, with explicit attribution.
    AddUrls {
        /// Folder ID in the lock.
        id: String,
        /// `model`, `texture`, `hdri` or `character`.
        #[arg(long, default_value = "model")]
        kind: String,
        #[arg(long)]
        title: String,
        /// Source page.
        #[arg(long)]
        source: String,
        /// SPDX license (`CC0-1.0`, `CC-BY-4.0`).
        #[arg(long)]
        license: String,
        /// Credited authors (comma-separated).
        #[arg(long, value_delimiter = ',')]
        authors: Vec<String>,
        /// Projects that use it (comma-separated).
        #[arg(long, value_delimiter = ',', required = true)]
        projects: Vec<String>,
        /// `path=url` pairs.
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Download and verify the assets of some projects (all by default).
    Fetch {
        /// Project folder names.
        projects: Vec<String>,
        /// Concurrent downloads.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
    },
    /// Check that every pinned file is present and verified; never downloads.
    Verify {
        /// Project folder names.
        projects: Vec<String>,
    },
    /// Regenerate `THIRD_PARTY_ASSETS.md` from the lock.
    Credits,
    /// Author a project's levels through the `ara` CLI (`outpost` or `showcase`).
    /// Needs fetched assets and release builds of `ara` and `ara-import-worker`.
    Build {
        /// Project folder name.
        project: String,
    },
    /// Write a project's generated, git-ignored kit files (`kit/`) without
    /// authoring any document. Needs fetched assets; projects without kits are a no-op.
    Kits {
        /// Project folder name.
        project: String,
    },
    /// Fidelity captures and GPU timings of one look (`<project>/look/<look>.look.json`).
    Capture {
        /// Project folder name.
        project: String,
        /// Look name.
        look: String,
        /// First shot only, native only (look iteration).
        #[arg(long)]
        quick: bool,
        /// Frames per timing window.
        #[arg(long, default_value_t = 240)]
        timing_frames: u32,
        /// Quality presets to capture: comma-separated low, medium, high, epic, or all.
        #[arg(long, default_value = "high")]
        quality: String,
        /// Path-traced reference samples per pixel for RMSE/FLIP scoring (0 skips).
        #[arg(long, default_value_t = 0)]
        reference: u32,
        /// Capture Medium and above with the opt-in ray-traced renderer (D27:
        /// every preset is raster by default).
        #[arg(long)]
        ray_tracing: bool,
    },
    /// Play a scene in a windowed `ara-player` through `ara mcp` and capture it.
    Play {
        /// Project folder name.
        project: String,
        /// Scene path (`levels/outpost.scene`).
        scene: String,
        /// Capture file stem.
        name: String,
        /// Ticks to walk forward before capturing.
        #[arg(long, default_value_t = 90)]
        walk: u32,
        /// A headless player instead of a window.
        #[arg(long)]
        headless: bool,
    },
    /// Open a scene in `ara-editor`, frame it over the Engine Protocol and capture.
    Editor {
        /// Project folder name.
        project: String,
        /// Scene path.
        scene: String,
        /// Capture file stem.
        name: String,
        /// Orbit target, yaw (rad), pitch (rad), distance: `x,y,z,yaw,pitch,distance`.
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        camera: Vec<f32>,
    },
}

fn kind(name: &str) -> Result<lock::Kind> {
    serde_json::from_value(serde_json::Value::String(name.to_owned()))
        .with_context(|| format!("unknown kind {name}"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let samples = cli
        .samples
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../samples")));
    let lock_path = samples.join("assets.lock.json");
    let read_lock = || -> Result<lock::Lock> {
        if lock_path.exists() {
            lock::Lock::read(&lock_path)
        } else {
            Ok(lock::Lock {
                version: 1,
                assets: Vec::new(),
            })
        }
    };
    match cli.command {
        Command::Game => game::author(&build::Ctx::open(
            &samples,
            "outpost",
            "Outpost",
            read_lock()?,
        )?)?,
        Command::AddPolyhaven {
            polyhaven,
            id,
            kind: k,
            resolution,
            projects,
        } => {
            let mut lock = read_lock()?;
            let id = id.unwrap_or_else(|| polyhaven.to_ascii_lowercase());
            sources::add(
                &mut lock,
                &fetch::Cache::new(&samples),
                &id,
                kind(&k)?,
                projects,
                sources::Source::PolyHaven {
                    id: polyhaven,
                    resolution,
                },
            )?;
            lock.write(&lock_path)?;
        }
        Command::AddUrls {
            id,
            kind: k,
            title,
            source,
            license,
            authors,
            projects,
            files,
        } => {
            let mut lock = read_lock()?;
            let files = files
                .iter()
                .map(|pair| {
                    pair.split_once('=')
                        .map(|(p, u)| (p.to_owned(), u.to_owned()))
                        .with_context(|| format!("expected path=url, got {pair}"))
                })
                .collect::<Result<_>>()?;
            sources::add(
                &mut lock,
                &fetch::Cache::new(&samples),
                &id,
                kind(&k)?,
                projects,
                sources::Source::Urls {
                    title,
                    source,
                    license,
                    authors,
                    files,
                },
            )?;
            lock.write(&lock_path)?;
        }
        Command::Fetch { projects, jobs } => {
            fetch::fetch(&samples, &read_lock()?, &projects, jobs)?;
            println!("fetched and verified");
        }
        Command::Verify { projects } => {
            let missing = fetch::verify(&samples, &read_lock()?, &projects)?;
            if !missing.is_empty() {
                anyhow::bail!(
                    "{} file(s) missing or modified; run `ara-samples fetch`:\n{}",
                    missing.len(),
                    missing.join("\n")
                );
            }
            println!("all pinned files present and verified");
        }
        Command::Credits => {
            let text = credits::markdown(&read_lock()?);
            std::fs::write(samples.join("THIRD_PARTY_ASSETS.md"), text)?;
        }
        Command::Build { project } => {
            let lock = read_lock()?;
            match project.as_str() {
                "outpost" => {
                    let ctx = build::Ctx::open(&samples, "outpost", "Outpost", lock)?;
                    build::outpost::build(&ctx)?;
                    game::author(&ctx)?;
                }
                "showcase" => build::showcase::build(&build::Ctx::open(
                    &samples,
                    "showcase",
                    "Material and character showcase",
                    lock,
                )?)?,
                other => anyhow::bail!("unknown project {other} (outpost, showcase)"),
            }
            println!("built {project}");
        }
        Command::Kits { project } => {
            let lock = read_lock()?;
            let root = samples.join(&project);
            match project.as_str() {
                "outpost" => build::outpost::generate(&root, &lock)?,
                "showcase" => build::showcase::generate(&root, &lock)?,
                _ => {
                    println!("{project} has no generated kits");
                    return Ok(());
                }
            }
            println!("generated {project} kits");
        }
        Command::Capture {
            project,
            look,
            quick,
            timing_frames,
            quality,
            reference,
            ray_tracing,
        } => {
            use ara_render_fidelity::presets::{QualityChoice, QualityPreset};
            let presets = if quality == "all" {
                QualityPreset::ALL.to_vec()
            } else {
                quality
                    .split(',')
                    .map(|q| match q.trim().parse()? {
                        QualityChoice::Preset(p) => Ok(p),
                        QualityChoice::Auto => anyhow::bail!("capture needs explicit presets"),
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
            };
            capture::run(
                &samples,
                &read_lock()?,
                &capture::Request {
                    project: &project,
                    look: &look,
                    quick,
                    timing_frames,
                    presets: &presets,
                    reference_samples: reference,
                    ray_tracing,
                },
            )?
        }
        Command::Play {
            project,
            scene,
            name,
            walk,
            headless,
        } => {
            let report = play::run(
                &samples,
                &play::Request {
                    project: &project,
                    scene: &scene,
                    name: &name,
                    walk,
                    window: !headless,
                },
            )?;
            let path = evidence_directory(&project).join(format!("{name}-player.json"));
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&report)?
                    + "
",
            )?;
            println!("{}", serde_json::to_string_pretty(&report["status"])?);
        }
        Command::Editor {
            project,
            scene,
            name,
            camera,
        } => {
            anyhow::ensure!(camera.len() == 6, "--camera takes x,y,z,yaw,pitch,distance");
            let report = editor::run(
                &samples,
                &editor::Request {
                    project: &project,
                    scene: &scene,
                    name: &name,
                    camera: (
                        [camera[0], camera[1], camera[2]],
                        camera[3],
                        camera[4],
                        camera[5],
                    ),
                },
            )?;
            let path = evidence_directory(&project).join(format!("{name}-editor.json"));
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&report)?
                    + "
",
            )?;
            println!("{}", report["logs"]["text"].as_str().unwrap_or_default());
        }
    }
    Ok(())
}
