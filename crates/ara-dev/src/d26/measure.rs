//! The scoreboard's measurements: each step runs an existing tool and keeps its
//! output (or why it failed) for `rows`.
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, ensure};
use serde_json::Value;

use super::{Options, Section};
use crate::binaries::Binaries;
use crate::perf::{self, Preset};

/// A sample scene: the pack it freezes into and the capture look that frames it.
pub(super) struct Scene {
    pub name: &'static str,
    /// Project folder under `samples/`.
    pub project: &'static str,
    /// The scene the pack's player opens (made the main scene of a staging copy
    /// when it is not the project's own).
    pub main: &'static str,
    pub look: &'static str,
}

pub(super) const SCENES: [Scene; 3] = [
    Scene {
        name: "Outpost",
        project: "outpost",
        main: "levels/outpost.scene",
        look: "outpost",
    },
    Scene {
        name: "Hangar",
        project: "outpost",
        main: "levels/hangar.scene",
        look: "hangar",
    },
    Scene {
        name: "Showcase",
        project: "showcase",
        main: "levels/showcase.scene",
        look: "showcase",
    },
];

/// A measurement, or why there is none.
pub(super) type Measured<T> = Result<T, String>;

/// One perf window: its `report.json` and the player's metrics in it.
pub(super) struct Window {
    pub report: PathBuf,
    pub player: Value,
}

/// A JSON file a tool wrote, and its contents.
pub(super) struct Output {
    pub path: PathBuf,
    pub value: Value,
}

/// Everything measured so far, keyed by scene name (and preset).
#[derive(Default)]
pub(super) struct Results {
    /// The cold High window per scene: a fresh pipeline cache, only the first frame
    /// skipped (player startup, cold; hitches after the first frame).
    pub cold: BTreeMap<&'static str, Measured<Window>>,
    /// Steady windows per (scene, preset).
    pub steady: BTreeMap<(&'static str, &'static str), Measured<Window>>,
    /// `ara-samples capture` timings per scene (ꟻLIP/RMSE per preset and shot).
    pub flip: BTreeMap<&'static str, Measured<Output>>,
    /// The editor probe on a fresh Outpost copy (`--settle-imports after`), then on
    /// the same copy again (`--settle-imports before`).
    pub editor_cold: Option<Measured<Output>>,
    pub editor_warm: Option<Measured<Output>>,
    /// The GPU the editor reported.
    pub device: Value,
    /// Sections left out, and why.
    pub skipped: BTreeMap<Section, String>,
}

/// What every step needs.
pub(super) struct Context<'a> {
    pub root: &'a Path,
    pub output: &'a Path,
    pub binaries: &'a Binaries,
    pub options: &'a Options,
}

fn error(error: anyhow::Error) -> String {
    format!("{error:#}")
}

impl Context<'_> {
    fn binary(&self, name: &str) -> Result<&Path> {
        Ok(self
            .binaries
            .get(name)
            .with_context(|| format!("the build did not produce {name}"))?)
    }

    /// Player windows: per scene, a cold High window, then a steady window per preset.
    pub fn player(&self, results: &mut Results) {
        let options = self.options;
        for scene in &SCENES {
            let pack = self.pack(scene);
            let cache = self.output.join("gpu-cache").join(scene.name);
            let window = |preset: Preset, warmup: u32, label: &str| -> Measured<Window> {
                let pack = pack.as_ref().map_err(Clone::clone)?;
                eprintln!("D26: {} {label}", scene.name);
                let options = perf::Options::window(
                    preset,
                    options.frames,
                    warmup,
                    &options.profile,
                    cache.clone(),
                );
                let directory = self
                    .output
                    .join("player")
                    .join(format!("{}-{label}", scene.name.to_lowercase()));
                let report = perf::session(
                    self.root,
                    &options,
                    self.binary("ara-player").map_err(error)?,
                    pack,
                    &directory,
                    0.,
                    Instant::now(),
                )
                .map_err(error)?;
                let value: Value =
                    serde_json::from_slice(&fs::read(&report).map_err(|e| error(e.into()))?)
                        .map_err(|e| error(e.into()))?;
                Ok(Window {
                    report,
                    player: value["player"].clone(),
                })
            };
            let cold = window(Preset::High, 1, "high-cold");
            let steady: Vec<_> = Preset::ALL
                .into_iter()
                .map(|preset| (preset, window(preset, options.warmup, preset.name())))
                .collect();
            results.cold.insert(scene.name, cold);
            for (preset, measured) in steady {
                results.steady.insert((scene.name, preset.name()), measured);
            }
        }
    }

    /// The scene's frozen pack, exported once per sample tree.
    fn pack(&self, scene: &Scene) -> Measured<PathBuf> {
        (|| -> Result<PathBuf> {
            perf::freeze::preflight(self.root, scene.project)?;
            let folder = Path::new("samples").join(scene.project);
            let tree = crate::git::text(
                self.root,
                [
                    "rev-parse",
                    &format!("HEAD:{}", folder.to_string_lossy().replace('\\', "/")),
                ],
            )?;
            let packs = self.root.join("target/d26/packs");
            let frozen = packs.join(format!(
                "{}-{}",
                scene.name.to_lowercase(),
                &tree[..12.min(tree.len())]
            ));
            if self.options.refreeze && frozen.exists() {
                fs::remove_dir_all(&frozen)?;
            }
            let mut project = self.root.join(&folder);
            let project_file: Value =
                serde_json::from_slice(&fs::read(project.join("ara.project.json"))?)?;
            let staging = packs.join(format!("{}-source", scene.name.to_lowercase()));
            if project_file["main"] != scene.main && !frozen.join("ara-launch.json").is_file() {
                // Another scene of the project: export a copy whose main scene it is.
                let _ = fs::remove_dir_all(&staging);
                super::copy_project(&project, &staging)?;
                let mut file = project_file.clone();
                file["main"] = Value::from(scene.main);
                fs::write(
                    staging.join("ara.project.json"),
                    serde_json::to_vec_pretty(&file)?,
                )?;
                project = staging.clone();
            }
            let packed = perf::freeze::pack(
                self.root,
                &self.root.join("target/ara-dev/perf"),
                &frozen,
                &project,
                self.binaries,
            );
            let _ = fs::remove_dir_all(&staging);
            packed
        })()
        .map_err(error)
    }

    /// ꟻLIP/RMSE per scene: the visual sweep's capture tool, or `--flip-from`.
    pub fn flip(&self, results: &mut Results) {
        for scene in &SCENES {
            let measured = match &self.options.flip_from {
                Some(folder) => timings(folder, scene),
                None => self.capture(scene),
            };
            results.flip.insert(scene.name, measured);
        }
    }

    fn capture(&self, scene: &Scene) -> Measured<Output> {
        let folder = self.output.join("captures");
        (|| -> Result<()> {
            fs::create_dir_all(&folder)?;
            let log = folder.join(format!("{}-{}.log", scene.project, scene.look));
            let mut command = Command::new(self.binary("ara-samples")?);
            command
                .current_dir(self.root)
                .args(["capture", scene.project, scene.look, "--quality", "all"])
                .args(["--reference", &self.options.reference.to_string()])
                .env("ARA_CAPTURE_OUTPUT_DIR", &folder)
                .env("ARA_IMPORT_WORKER", self.binary("ara-import-worker")?)
                .env("VK_INSTANCE_LAYERS", "")
                .stdout(File::create(&log)?)
                .stderr(File::create(log.with_extension("stderr.log"))?);
            eprintln!("D26: capturing {} / {} (ꟻLIP)", scene.project, scene.look);
            run(&mut command, Duration::from_secs(2 * 3600), &log)
        })()
        .map_err(error)?;
        timings(&folder, scene)
    }

    /// The editor probe: a fresh Outpost copy (cold: interactions during imports,
    /// then a wait for them), then the same copy again (warm, after imports).
    pub fn editor(&self, results: &mut Results) {
        let project = self.output.join("editor/outpost-project");
        let copied = super::copy_project(&self.root.join("samples/outpost"), &project);
        let probe = |settle: &str| -> Measured<Output> {
            copied
                .as_ref()
                .map_err(|e| format!("copying the project: {e:#}"))?;
            self.probe(&project, settle).map_err(error)
        };
        let cold = probe("after");
        let warm = probe("before");
        for measured in [&cold, &warm] {
            if let Ok(output) = measured
                && results.device.is_null()
            {
                results.device = output.value["device"].clone();
            }
        }
        results.editor_cold = Some(cold);
        results.editor_warm = Some(warm);
        if !self.options.keep_project {
            let _ = fs::remove_dir_all(&project);
        }
    }

    fn probe(&self, project: &Path, settle: &str) -> Result<Output> {
        let label = if settle == "after" { "cold" } else { "warm" };
        let output = self.output.join("editor").join(label);
        let layers = output.join("empty-vulkan-layers");
        fs::create_dir_all(&layers)?;
        let log = output.join("editor.log");
        let mut command = Command::new(self.binary("ara-editor")?);
        command
            .current_dir(self.root)
            .arg(project)
            .args(["--verify-responsiveness", "--settle-imports", settle])
            .args(["--size", "2560x1440", "--output"])
            .arg(&output)
            .env("ARA_PLAYER", self.binary("ara-player")?)
            .env("ARA_SHADER_DIR", self.root)
            .env("VK_INSTANCE_LAYERS", "")
            .env("VK_IMPLICIT_LAYER_PATH", &layers)
            .stdout(File::create(&log)?)
            .stderr(File::create(log.with_extension("stderr.log"))?);
        eprintln!("D26: editor probe, {label}");
        run(&mut command, Duration::from_secs(45 * 60), &log)?;
        let path = output.join("responsiveness.json");
        Ok(Output {
            value: serde_json::from_slice(&fs::read(&path)?)?,
            path,
        })
    }
}

/// Run a tool under the GPU lock, bounded by `limit`.
fn run(command: &mut Command, limit: Duration, log: &Path) -> Result<()> {
    crate::gpu::locked(4 * 3600, || {
        let outcome = crate::process::run(command, Some(Instant::now() + limit))?;
        ensure!(
            outcome.success(),
            "failed or timed out after {} s; see {}",
            limit.as_secs(),
            log.display()
        );
        Ok(())
    })
}

/// The capture tool's timings for a scene's look under `folder`.
fn timings(folder: &Path, scene: &Scene) -> Measured<Output> {
    let path = folder
        .join(scene.project)
        .join(scene.look)
        .join("evidence")
        .join(format!("{}-timings.json", scene.look));
    let bytes =
        fs::read(&path).map_err(|e| format!("no capture timings at {}: {e}", path.display()))?;
    let value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Output { path, value })
}
