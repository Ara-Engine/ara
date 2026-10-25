//! Build > Build Current Profile: export the project as a real game (the packed
//! runtime player) through `ara-export` on a worker thread.
//! - The profile is `presets/<name>.export.json`. Without one, Build creates the
//!   portable default (`build.profile.create`: Windows, real runtime, plain folder).
//!   Profiles hold no machine paths: the runtime template (this engine's
//!   ara-player.exe, its PDB and the current shader cohorts) is zipped and pinned at
//!   build time under `.ara/export/`.
//! - The status bar shows the stage and cooking progress; export diagnostics go to
//!   the bus with their `ARA-EXP-*` codes, so Problems explains them.
//! - The result lands in `builds/<profile>-<time>/`; Run Build launches it (one
//!   headless frame with a capture in offscreen runs, never a window).
//! - C# modules (`scripts/*/ara.module.toml`) ship with the game: the export finds
//!   them and publishes them NativeAOT (needs the .NET SDK on the building machine).
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use super::*;

/// The profile Build Current Profile creates when the project has none.
pub(crate) const DEFAULT_PROFILE: &str = "windows";

/// What the export thread reports to the window.
pub(crate) enum BuildEvent {
    /// A stage started (`template`, `content`, `assemble`, `verify`, ...).
    Stage(String),
    /// Cooking progress: completed and requested cooks.
    Cooking(usize, usize),
    /// A warning or error, as bus-ready diagnostic JSON.
    Diagnostic(Value),
    /// The export ended: the folder holding the game, or why it failed.
    Finished(Result<Built, String>),
}

/// A finished export.
pub(crate) struct Built {
    /// The folder with ara-player.exe, ara-launch.json and game.arapack.
    pub game: PathBuf,
    /// Seconds the export took.
    pub seconds: f64,
    /// The export's own verification ran the game (not only static checks).
    pub executed: bool,
}

/// A running export.
pub(crate) struct BuildJob {
    events: Receiver<BuildEvent>,
    /// Profile name, for the status bar.
    pub preset: String,
    /// Current stage.
    pub stage: String,
    /// Cooking progress (completed, requested), while content cooks.
    pub cooking: Option<(usize, usize)>,
}

/// A launched build: what it printed, or why it did not start.
pub(crate) type RunResult = Result<String, String>;

struct Sink(Sender<BuildEvent>);

impl ara_export::event::EventSink for Sink {
    fn emit(&mut self, event: &ara_export::event::Event) -> std::io::Result<()> {
        use ara_export::event::EventKind;
        let message = match &event.kind {
            EventKind::Progress { status, .. } if status == "started" => {
                Some(BuildEvent::Stage(event.stage.clone()))
            }
            EventKind::Cooking {
                completed, total, ..
            } => Some(BuildEvent::Cooking(*completed, *total)),
            EventKind::Warning(d) | EventKind::Error(d) => {
                let error = matches!(event.kind, EventKind::Error(_));
                let code = ara_diag::catalog::lookup(&d.code)
                    .map(|doc| doc.code)
                    .unwrap_or(crate::codes::BUILD);
                let mut message = d.message.clone();
                if let Some(fix) = d.fixes.first() {
                    message.push_str(&format!(" Fix: {fix}"));
                }
                let diagnostic = if error {
                    ara_diag::Diagnostic::error(code, message)
                } else {
                    ara_diag::Diagnostic::warning(code, message)
                }
                .subject(ara_diag::Subject::system("export"));
                serde_json::to_value(&diagnostic)
                    .ok()
                    .map(BuildEvent::Diagnostic)
            }
            _ => None,
        };
        if let Some(message) = message {
            let _ = self.0.send(message);
        }
        Ok(())
    }
}

/// The project's export profiles (`presets/*.export.json`), by name.
pub(crate) fn presets(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("presets"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".export.json")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// Log a build message (ARA4505 unless it carries an export code) on the bus, so
/// Console and Problems show it.
fn log(editor: &mut Editor, diagnostic: ara_diag::Diagnostic) {
    let diagnostic = diagnostic.subject(ara_diag::Subject::system("export"));
    if let Ok(value) = serde_json::to_value(&diagnostic) {
        let _ = editor.action(
            "diag.report",
            json!({"diagnostics": [value], "engine": true}),
        );
    }
}

/// The portable default profile: Windows, the real game runtime, a plain folder; C#
/// modules are found by the export. Its runtime template is pinned at build time
/// (`runtime_template`), so the file is unpinned (`ExportPreset::to_unpinned_text`).
fn default_profile() -> anyhow::Result<ara_export::ExportPreset> {
    let preset: ara_export::ExportPreset = serde_json::from_value(json!({
        "platform": "windows",
        "config": "dev",
        "lane": "nativeAot",
        "playerTemplate": "runtime",
        "scriptProject": "",
        "assembly": "AraGame",
        "content": "assets",
        "templates": [],
    }))?;
    Ok(preset)
}

/// The engine the editor runs from: an installed engine (`resources/<package>` beside
/// the executable) or a checkout (`crates/<package>/cooked` above it).
fn engine_root(exe_dir: &Path) -> Option<PathBuf> {
    if exe_dir.join("resources/ara-render").is_dir() {
        return Some(exe_dir.into());
    }
    exe_dir
        .ancestors()
        .find(|dir| dir.join("crates/ara-render/cooked").is_dir())
        .map(Into::into)
}

fn pin(path: &Path) -> anyhow::Result<ara_export::preset::ArtifactPin> {
    use anyhow::Context;
    let source = std::path::absolute(path)?;
    Ok(ara_export::preset::ArtifactPin {
        hash: blake3::hash(&std::fs::read(&source)?).to_string(),
        source: source
            .to_str()
            .context("the template path is not UTF-8")?
            .to_owned(),
    })
}

/// Zips this engine's player and current shader cohorts into the project's
/// `.ara/export/` and pins it (with the player's PDB) for `config`.
fn runtime_template(
    project: &Path,
    config: ara_export::preset::Configuration,
) -> anyhow::Result<ara_export::preset::TemplatePin> {
    use anyhow::{Context, ensure};
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("the editor's folder")?;
    let player = dir.join("ara-player.exe");
    ensure!(
        player.is_file(),
        "ara-player.exe is not next to the editor ({}); reinstall the engine",
        dir.display()
    );
    let symbols = dir.join("ara_player.pdb");
    ensure!(
        symbols.is_file(),
        "ara_player.pdb (the player's symbols) is not next to ara-player.exe; Windows builds pin it with the player"
    );
    let engine = engine_root(dir).context(
        "the engine's cooked shader resources were not found (resources/ beside the editor, or crates/*/cooked in a checkout)",
    )?;
    let zip = project.join(".ara/export/templates/runtime-windows-x64.zip");
    // Rebuilt each time: the player or its shader cohorts may have changed.
    let _ = std::fs::remove_file(&zip);
    ara_export::build_runtime_template(&player, &engine, &zip)?;
    let template = pin(&zip)?;
    Ok(ara_export::preset::TemplatePin {
        platform: ara_export::preset::Platform::Windows,
        arch: "x64".into(),
        config,
        hash: template.hash,
        source: template.source,
        symbols: Some(pin(&symbols)?),
    })
}

/// Exports `preset` (its runtime template filled in) into `output`.
fn export(
    root: PathBuf,
    mut preset: ara_export::ExportPreset,
    output: PathBuf,
    send: &Sender<BuildEvent>,
) -> anyhow::Result<Built> {
    let started = std::time::Instant::now();
    if preset.player_template == ara_export::preset::PlayerTemplate::Runtime
        && preset.templates.is_empty()
    {
        let _ = send.send(BuildEvent::Stage("runtime template".into()));
        preset.templates = vec![runtime_template(&root, preset.config)?];
    }
    let request = ara_export::ExportRequest {
        project: root.clone(),
        preset,
        output: output.clone(),
        cache: root.join(".ara/export/cache"),
        device: None,
        allow_emulator: false,
    };
    let summary = ara_export::export(&request, &mut Sink(send.clone()))?;
    Ok(Built {
        game: output.join("build"),
        seconds: started.elapsed().as_secs_f64(),
        executed: summary.verify.runtime_executed,
    })
}

impl Shell {
    /// Build > Create Build Profile: write the portable default profile
    /// (`presets/windows.export.json`) with the canonical writer. Returns its name.
    pub(crate) fn create_build_profile(&mut self, editor: &mut Editor) -> ara_ui::Result<String> {
        let root = editor.host.root().to_owned();
        let path = root
            .join("presets")
            .join(format!("{DEFAULT_PROFILE}.export.json"));
        if path.exists() {
            return Err(format!("presets/{DEFAULT_PROFILE}.export.json already exists").into());
        }
        let text = default_profile()
            .and_then(|p| p.to_unpinned_text())
            .map_err(|e| format!("{e:#}"))?;
        std::fs::create_dir_all(root.join("presets"))?;
        std::fs::write(&path, text)?;
        log(
            editor,
            ara_diag::Diagnostic::info(
                crate::codes::BUILD,
                format!(
                    "Created the build profile presets/{DEFAULT_PROFILE}.export.json (Windows, the real game runtime, a plain folder)"
                ),
            ),
        );
        Ok(DEFAULT_PROFILE.into())
    }

    /// Start exporting with the first profile, creating the default one when the
    /// project has none (Build > Build Current Profile).
    pub(crate) fn build_current(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.build.is_some() {
            return Err("an export is already running".into());
        }
        // The host's root is canonical (`\\?\C:\…` on Windows); build tools such as
        // MSBuild cannot evaluate verbatim paths, so the export gets the plain form.
        let root = dunce::simplified(editor.host.root()).to_path_buf();
        let preset = match presets(&root).into_iter().next() {
            Some(preset) => preset,
            None => self.create_build_profile(editor)?,
        };
        editor.host.flush();
        let text =
            std::fs::read_to_string(root.join("presets").join(format!("{preset}.export.json")))?;
        let parsed =
            ara_export::ExportPreset::from_unpinned_text(&text).map_err(|e| format!("{e:#}"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let output = root.join("builds").join(format!("{preset}-{stamp}"));
        let (send, events) = channel();
        std::thread::spawn(move || {
            let result = export(root, parsed, output, &send).map_err(|e| format!("{e:#}"));
            let _ = send.send(BuildEvent::Finished(result));
        });
        self.build = Some(BuildJob {
            events,
            preset: preset.clone(),
            stage: "starting".into(),
            cooking: None,
        });
        self.status_note = Some(format!("Building {preset}…"));
        Ok(())
    }

    /// Build > Run Build: launch the last build. Offscreen (verification) runs step
    /// it headless for one tick and capture `run.png`; never a window.
    pub(crate) fn run_build(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let game = self
            .last_build
            .clone()
            .ok_or("Build the project first (Build Current Profile)")?;
        let player = game.join("ara-player.exe");
        let mut command = std::process::Command::new(&player);
        command.current_dir(&game).arg("--root").arg(&game);
        if self.offscreen {
            let capture = game.join("run.png");
            command
                .args(["--headless", "--ticks", "1", "--capture"])
                .arg(&capture);
            let (send, results) = channel();
            std::thread::spawn(move || {
                let result = command
                    .output()
                    .map_err(|e| format!("{} did not start: {e}", player.display()))
                    .and_then(|out| {
                        let text = String::from_utf8_lossy(&out.stderr).into_owned();
                        if out.status.success() && capture.is_file() {
                            Ok(capture.display().to_string())
                        } else {
                            let tail: String = text
                                .lines()
                                .rev()
                                .take(6)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join(" · ");
                            Err(format!("the build exited with {}: {tail}", out.status))
                        }
                    });
                let _ = send.send(result);
            });
            self.run = Some(results);
            self.status_note = Some("Running the build headless for one frame…".into());
        } else {
            command
                .spawn()
                .map_err(|e| format!("{} did not start: {e}", player.display()))?;
            self.status_note = Some(format!("Started {}", player.display()));
        }
        let _ = editor;
        Ok(())
    }

    /// Drain export and run events at a frame boundary; true when the UI should rebuild.
    pub(crate) fn poll_build(&mut self, editor: &mut Editor) -> bool {
        let mut changed = false;
        if let Some(results) = &self.run
            && let Ok(result) = results.try_recv()
        {
            self.run = None;
            changed = true;
            match result {
                Ok(capture) => {
                    self.last_run = Some(PathBuf::from(&capture));
                    let shown = dunce::simplified(std::path::Path::new(&capture))
                        .strip_prefix(dunce::simplified(editor.host.root()))
                        .map_or_else(
                            |_| capture.clone(),
                            |p| p.to_string_lossy().replace('\\', "/"),
                        );
                    self.status_note = Some(format!("The build ran one frame: {shown}"));
                    log(
                        editor,
                        ara_diag::Diagnostic::info(
                            crate::codes::BUILD,
                            format!("The build ran one frame and captured {capture}"),
                        ),
                    );
                }
                Err(error) => {
                    self.status_note = Some("The build did not run: see Problems".into());
                    log(
                        editor,
                        ara_diag::Diagnostic::error(
                            crate::codes::BUILD,
                            format!("Run Build failed: {error}"),
                        ),
                    );
                }
            }
        }
        let Some(job) = &mut self.build else {
            return changed;
        };
        let mut finished = None;
        while let Ok(event) = job.events.try_recv() {
            changed = true;
            match event {
                BuildEvent::Stage(stage) => {
                    job.stage = stage;
                    job.cooking = None;
                }
                BuildEvent::Cooking(completed, total) => job.cooking = Some((completed, total)),
                BuildEvent::Diagnostic(diagnostic) => {
                    let _ = editor.action(
                        "diag.report",
                        json!({"diagnostics": [diagnostic], "engine": true}),
                    );
                }
                BuildEvent::Finished(result) => finished = Some(result),
            }
        }
        if let Some(result) = finished {
            let preset = job.preset.clone();
            self.build = None;
            match result {
                Ok(built) => {
                    let ran = if built.executed {
                        "; it ran in verification"
                    } else {
                        ""
                    };
                    self.status_note = Some(format!(
                        "Built {preset} in {:.0} s{ran}: {}",
                        built.seconds,
                        built.game.display()
                    ));
                    log(
                        editor,
                        ara_diag::Diagnostic::info(
                            crate::codes::BUILD,
                            format!(
                                "Built {preset} in {:.0} s{ran}: {}",
                                built.seconds,
                                built.game.display()
                            ),
                        ),
                    );
                    self.last_build = Some(built.game);
                }
                Err(error) => {
                    self.status_note = Some("The build failed: see Problems".into());
                    log(
                        editor,
                        ara_diag::Diagnostic::error(
                            crate::codes::BUILD,
                            format!("Build failed: {error}"),
                        ),
                    );
                }
            }
        }
        changed
    }
}
