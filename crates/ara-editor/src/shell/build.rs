//! Build > Build Current Profile: export the project as a real game (the packed
//! runtime player) through `ara-export` on a worker thread.
//! - The profile is `settings/builds/<name>.export.json` (legacy `presets/` works).
//!   Without one, Build creates the
//!   portable default (`build.profile.create`: Windows, real runtime, plain folder).
//!   Profiles hold no machine paths: the runtime template (this engine's
//!   ara-player.exe, its PDB and the current shader cohorts) is zipped and pinned at
//!   build time under `.ara/export/`.
//! - The status bar shows the stage and cooking progress; export diagnostics go to
//!   the bus with their `ARA-EXP-*` codes, so Problems explains them.
//! - The result lands in `builds/<profile>-<time>/`; Run Build launches it (one
//!   headless frame with a capture in offscreen runs, never a window).
//! - C# modules anywhere beneath the shared authored root ship with the game: the export finds
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
    /// The shipped folder with the named application, its data and notices.
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

/// Named export profiles, with settings/builds taking precedence over legacy presets.
pub(crate) fn presets(root: &Path) -> Vec<String> {
    for directory in ["settings/builds", "presets"] {
        let mut names: Vec<String> = std::fs::read_dir(root.join(directory))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .strip_suffix(".export.json")
                    .map(str::to_owned)
            })
            .collect();
        if !names.is_empty() {
            names.sort();
            return names;
        }
    }
    Vec::new()
}

fn profile_path(root: &Path, name: &str) -> PathBuf {
    let current = root
        .join("settings/builds")
        .join(format!("{name}.export.json"));
    if current.is_file() {
        current
    } else {
        root.join("presets").join(format!("{name}.export.json"))
    }
}

struct Launch {
    executable: PathBuf,
    data: PathBuf,
    report: PathBuf,
}

/// Resolve the actual shipped entry point and data from the store-neutral manifest.
/// Canonical project-path validation rejects absolute paths and junction escapes.
fn launch(game: &Path) -> anyhow::Result<Launch> {
    use anyhow::{Context, ensure};
    let game = dunce::canonicalize(game)?;
    let output = game.parent().context("the export has no report folder")?;
    let manifest: ara_export::distribution::DistributionManifest =
        serde_json::from_slice(&std::fs::read(output.join("distribution.json"))?)?;
    ensure!(
        manifest.format == "ara.distribution/1"
            && manifest.files.contains_key(&manifest.entry_point),
        "the export manifest does not identify a shipped application"
    );
    let executable = ara_export::tools::project_path(&game, &manifest.entry_point)?;
    ensure!(
        executable.is_file(),
        "the exported game executable is missing"
    );
    let data = if manifest.platform == "macos" {
        let contents = executable
            .parent()
            .and_then(Path::parent)
            .context("the macOS bundle has no Contents folder")?;
        let relative = contents
            .join("Resources")
            .strip_prefix(&game)?
            .to_path_buf();
        ara_export::tools::project_path(&game, &relative.to_string_lossy().replace('\\', "/"))?
    } else {
        let data = ara_gpu::cooked::player_data_root(&game);
        data
    };
    ensure!(
        data.is_dir() && data.join("ara-launch.json").is_file(),
        "the exported game launch configuration is missing"
    );
    std::fs::create_dir_all(output.join("report"))?;
    let report = ara_export::tools::project_path(output, "report")?;
    Ok(Launch {
        executable,
        data,
        report,
    })
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
        "content": ara_host::project_layout::AUTHORING_ROOT,
        "templates": [],
    }))?;
    Ok(preset)
}

/// The engine the editor runs from: an installed engine (`resources/<package>` beside
/// the executable) or a checkout (`crates/<package>/cooked` above it).
fn engine_root(exe_dir: &Path) -> Option<PathBuf> {
    if exe_dir.join("engine.json").is_file() || exe_dir.join("resources/ara-render").is_dir() {
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
    send: &Sender<BuildEvent>,
) -> anyhow::Result<ara_export::preset::TemplatePin> {
    use anyhow::Context;
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("the editor's folder")?;
    let player = dir.join("ara-player.exe");
    let symbols = dir.join("ara_player.pdb");
    let engine = engine_root(dir).unwrap_or_else(|| dir.to_owned());
    let _ = send.send(BuildEvent::Stage("checking engine installation".into()));
    if let Err(error) = ara_export::preflight::runtime(&player, &engine, Some(&symbols)) {
        let diagnostic = ara_export::preflight::diagnostic(&engine, &format!("{error:#}"));
        if let Ok(value) = serde_json::to_value(diagnostic) {
            let _ = send.send(BuildEvent::Diagnostic(value));
        }
        return Err(error);
    }
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
    stable_parent: Option<PathBuf>,
    send: &Sender<BuildEvent>,
) -> anyhow::Result<Built> {
    let started = std::time::Instant::now();
    if preset.player_template == ara_export::preset::PlayerTemplate::Runtime
        && preset.templates.is_empty()
    {
        let _ = send.send(BuildEvent::Stage("runtime template".into()));
        preset.templates = vec![runtime_template(&root, preset.config, send)?];
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
    let output = if let Some(parent) = stable_parent {
        publish_build(&output, &parent, &summary.build_id)?
    } else {
        output
    };
    Ok(Built {
        game: output.join("build"),
        seconds: started.elapsed().as_secs_f64(),
        executed: summary.verify.runtime_executed,
    })
}

/// Claim a new content-identified developer folder; an existing build is preserved.
fn publish_build(staged: &Path, parent: &Path, build_id: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        build_id.len() == 64 && build_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "the export returned an invalid build identity"
    );
    std::fs::create_dir_all(parent)?;
    let mut suffix = 1_u32;
    let destination = loop {
        let name = if suffix == 1 {
            build_id.to_owned()
        } else {
            format!("{build_id}-{suffix}")
        };
        let path = parent.join(name);
        match std::fs::create_dir(&path) {
            Ok(()) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
            Err(error) => return Err(error.into()),
        }
    };
    // The manifest moves last: the claimed folder is not a completed export until
    // every shipped file, report, package and symbol archive has moved into it.
    let mut entries = std::fs::read_dir(staged)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name() == "distribution.json");
    for entry in entries {
        std::fs::rename(entry.path(), destination.join(entry.file_name()))?;
    }
    std::fs::remove_dir(staged)?;
    Ok(destination)
}

impl Shell {
    /// Build > Create Build Profile: write the portable default profile
    /// (`settings/builds/windows.export.json`) with the canonical writer. Returns its name.
    pub(crate) fn create_build_profile(&mut self, editor: &mut Editor) -> ara_ui::Result<String> {
        let root = editor.host.root().to_owned();
        let path = root
            .join("settings/builds")
            .join(format!("{DEFAULT_PROFILE}.export.json"));
        if path.exists() {
            return Err(
                format!("settings/builds/{DEFAULT_PROFILE}.export.json already exists").into(),
            );
        }
        let text = default_profile()
            .and_then(|p| p.to_unpinned_text())
            .map_err(|e| format!("{e:#}"))?;
        std::fs::create_dir_all(root.join("settings/builds"))?;
        std::fs::write(&path, text)?;
        log(
            editor,
            ara_diag::Diagnostic::info(
                crate::codes::BUILD,
                format!(
                    "Created the build profile settings/builds/{DEFAULT_PROFILE}.export.json (Windows, the real game runtime, a plain folder)"
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
        let root = dunce::simplified(editor.host.root());
        let preset = match presets(root).into_iter().next() {
            Some(preset) => preset,
            None => self.create_build_profile(editor)?,
        };
        self.build_profile(editor, &preset, None)
    }

    /// Build a selected profile into a new chosen export folder, or by content ID
    /// under builds/<profile> when no destination is supplied. Existing outputs stay.
    pub(crate) fn build_profile(
        &mut self,
        editor: &mut Editor,
        profile: &str,
        destination: Option<PathBuf>,
    ) -> ara_ui::Result<()> {
        if self.build.is_some() {
            return Err("an export is already running".into());
        }
        // The host's root is canonical (`\\?\C:\…` on Windows); build tools such as
        // MSBuild cannot evaluate verbatim paths, so the export gets the plain form.
        let root = dunce::simplified(editor.host.root()).to_path_buf();
        if !presets(&root).iter().any(|name| name == profile) {
            return Err(format!(
                "unknown build profile {profile}; choose a profile from settings/builds/"
            )
            .into());
        }
        let preset = profile.to_owned();
        editor.host.flush();
        let text = std::fs::read_to_string(profile_path(&root, &preset))?;
        let parsed =
            ara_export::ExportPreset::from_unpinned_text(&text).map_err(|e| format!("{e:#}"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let stable_parent = destination
            .is_none()
            .then(|| root.join("builds").join(&preset));
        let output = destination
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                }
            })
            .unwrap_or_else(|| {
                root.join("builds")
                    .join(&preset)
                    .join(format!(".export-{}-{stamp}", std::process::id()))
            });
        if output.exists() {
            return Err("the build destination already exists; choose a new folder".into());
        }
        let (send, events) = channel();
        std::thread::spawn(move || {
            let result =
                export(root, parsed, output, stable_parent, &send).map_err(|e| format!("{e:#}"));
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
    /// it headless for one tick and capture `report/run.png` outside the shipped folder.
    pub(crate) fn run_build(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let game = self
            .last_build
            .clone()
            .ok_or("Build the project first (Build Current Profile)")?;
        let launch = launch(&game).map_err(|error| format!("{error:#}"))?;
        let player = launch.executable;
        let mut command = std::process::Command::new(&player);
        command
            .current_dir(&launch.report)
            .arg("--root")
            .arg(&launch.data);
        if self.offscreen {
            let capture = launch.report.join("run.png");
            command
                .args(["--headless", "--ticks", "1", "--capture"])
                .arg(&capture)
                .env("ARA_CONFIG_DIR", launch.report.join("run-profile"))
                .env("ARA_CRASH_DIR", launch.report.join("run-crashes"));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000 | 0x0000_4000); // CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> anyhow::Result<Self> {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/editor-build-layout");
            std::fs::create_dir_all(&root)?;
            let directory = root.join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory)?;
            Ok(Self(directory))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest(output: &Path, entry: &str, platform: &str) -> anyhow::Result<()> {
        ara_export::tools::write_json(
            &output.join("distribution.json"),
            &json!({
                "format":"ara.distribution/1", "buildId":"fixture", "platform":platform,
                "entryPoint":entry, "files":{entry:{"hash":"fixture","bytes":0,"executable":true}},
                "packages":{}, "symbols":"symbols/fixture.zip", "signing":"unsigned"
            }),
        )
    }

    #[test]
    fn settings_profiles_take_precedence_and_legacy_profiles_still_work() -> anyhow::Result<()> {
        let scratch = Scratch::new()?;
        let root = &scratch.0;
        std::fs::create_dir_all(root.join("presets"))?;
        std::fs::write(root.join("presets/legacy.export.json"), "legacy")?;
        assert_eq!(presets(root), ["legacy"]);
        assert_eq!(
            profile_path(root, "legacy"),
            root.join("presets/legacy.export.json")
        );
        std::fs::create_dir_all(root.join("settings/builds/not-a-profile.export.json"))?;
        std::fs::write(root.join("settings/builds/windows.export.json"), "current")?;
        std::fs::write(root.join("presets/windows.export.json"), "old")?;
        assert_eq!(presets(root), ["windows"]);
        assert_eq!(
            profile_path(root, "windows"),
            root.join("settings/builds/windows.export.json")
        );
        let default = default_profile()?;
        assert!(default.templates.is_empty());
        assert_eq!(default.content, ara_host::project_layout::AUTHORING_ROOT);
        assert!(
            ara_export::ExportPreset::from_unpinned_text(&default.to_unpinned_text()?)?
                .templates
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn run_build_resolves_a_relocated_named_application_and_external_reports() -> anyhow::Result<()>
    {
        let scratch = Scratch::new()?;
        let output = scratch.0.join("first");
        let game = output.join("build");
        std::fs::create_dir_all(game.join("data"))?;
        std::fs::write(game.join("Outpost.exe"), b"player fixture")?;
        ara_export::tools::write_json(
            &game.join("data/ara-launch.json"),
            &json!({"contract":"ara.player.runtime/1","pack":"game.arapack"}),
        )?;
        manifest(&output, "Outpost.exe", "windows")?;
        let moved = scratch.0.join("relocated");
        std::fs::rename(&output, &moved)?;
        let launch = launch(&moved.join("build"))?;
        assert_eq!(
            launch.executable,
            dunce::canonicalize(moved.join("build/Outpost.exe"))?
        );
        assert_eq!(launch.data, dunce::canonicalize(moved.join("build/data"))?);
        assert_eq!(launch.report, dunce::canonicalize(moved.join("report"))?);
        assert!(
            !launch
                .report
                .starts_with(dunce::canonicalize(moved.join("build"))?)
        );
        assert!(!moved.join("build/run.png").exists());
        for entry in ["../outside.exe", "C:/outside.exe", "data/../../outside.exe"] {
            manifest(&moved, entry, "windows")?;
            assert!(super::launch(&moved.join("build")).is_err(), "{entry}");
        }
        Ok(())
    }

    #[test]
    fn macos_manifest_locates_bundle_resources_without_assuming_the_players_name()
    -> anyhow::Result<()> {
        let scratch = Scratch::new()?;
        let game = scratch.0.join("build");
        std::fs::create_dir_all(game.join("Salvage.app/Contents/MacOS"))?;
        std::fs::create_dir_all(game.join("Salvage.app/Contents/Resources"))?;
        std::fs::write(
            game.join("Salvage.app/Contents/MacOS/Salvage"),
            b"player fixture",
        )?;
        std::fs::write(
            game.join("Salvage.app/Contents/Resources/ara-launch.json"),
            b"{}",
        )?;
        manifest(&scratch.0, "Salvage.app/Contents/MacOS/Salvage", "macos")?;
        let launch = launch(&game)?;
        assert_eq!(
            launch.data,
            dunce::canonicalize(game.join("Salvage.app/Contents/Resources"))?
        );
        assert!(launch.executable.ends_with("MacOS/Salvage"));
        Ok(())
    }

    #[test]
    fn content_identified_builds_preserve_an_existing_result() -> anyhow::Result<()> {
        let scratch = Scratch::new()?;
        let parent = scratch.0.join("builds/windows");
        let id = "a".repeat(64);
        for index in [1, 2] {
            let staged = scratch.0.join(format!("staged-{index}"));
            std::fs::create_dir_all(staged.join("build/data"))?;
            std::fs::write(staged.join("build/Outpost.exe"), format!("player {index}"))?;
            std::fs::write(staged.join("distribution.json"), b"fixture")?;
            let result = publish_build(&staged, &parent, &id)?;
            assert_eq!(
                result,
                parent.join(if index == 1 {
                    id.clone()
                } else {
                    format!("{id}-2")
                })
            );
            assert!(!staged.exists());
        }
        assert_eq!(
            std::fs::read_to_string(parent.join(&id).join("build/Outpost.exe"))?,
            "player 1"
        );
        Ok(())
    }
}
