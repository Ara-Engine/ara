//! `ara-dev live hub|editor`: keep a development Hub or editor open while you edit.
//! CSS and theme saves apply inside the running window (the apps' debug live styles).
//! Rust, manifest and other embedded-resource saves rebuild; a build that changed the
//! executables relaunches from a staged copy. Staging leaves Cargo free to relink on
//! Windows, and a failed build keeps the previous window running. Enter relaunches.
use std::ffi::OsString;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, ValueEnum};
use notify::Watcher;

#[derive(Args)]
pub(crate) struct Options {
    /// The app to keep running.
    app: App,
    /// Arguments for the app, after `--` (the editor takes a project directory).
    #[arg(last = true)]
    args: Vec<OsString>,
}

#[derive(Clone, Copy, ValueEnum)]
enum App {
    Hub,
    Editor,
}

impl App {
    fn name(self) -> &'static str {
        match self {
            Self::Hub => "ara-hub",
            Self::Editor => "ara-editor",
        }
    }
    /// Workspace-relative CSS this app reloads in place; saving it never rebuilds.
    fn live_css(self) -> &'static [&'static str] {
        match self {
            Self::Hub => &[
                "crates/ara-hub/src/view/hub.css",
                "crates/ara-ui/src/select/select.css",
            ],
            Self::Editor => &[
                "crates/ara-ui/src/dialogs/dialogs.css",
                "crates/ara-ui/src/select/select.css",
            ],
        }
    }
}

#[derive(PartialEq)]
enum Trigger {
    Save,
    Enter,
}

/// Cargo's executables: the app first, then the helpers it starts from beside itself.
struct Build {
    directory: PathBuf,
    executables: Vec<PathBuf>,
}

struct Running {
    tree: crate::process::Tree,
    stamps: Vec<(Option<SystemTime>, u64)>,
    directory: PathBuf,
}

struct Session<'a> {
    root: &'a Path,
    app: App,
    /// `target/ara-dev/live/<app>`: staged generations and the Hub's scratch profile.
    live: PathBuf,
    args: Vec<OsString>,
    running: Option<Running>,
    generation: u64,
}

pub(crate) fn run(root: &Path, options: Options) -> Result<bool> {
    let live = root.join("target/ara-dev/live").join(options.app.name());
    for generation in fs::read_dir(&live).into_iter().flatten().flatten() {
        if generation.file_name().to_string_lossy().starts_with("gen-") {
            let _ = fs::remove_dir_all(generation.path());
        }
    }
    let (send, triggers) = mpsc::channel();
    let _watcher = watch(root, options.app, send.clone())?;
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if line.is_err() || send.send(Trigger::Enter).is_err() {
                break;
            }
        }
    });
    let mut session = Session {
        root,
        app: options.app,
        live,
        args: options.args,
        running: None,
        generation: 0,
    };
    let mut forced = false;
    loop {
        let name = session.app.name();
        eprintln!("ara-dev live: building {name}");
        match session.build() {
            Err(error) => eprintln!(
                "ara-dev live: build failed{}: {error:#}",
                if session.running.is_some() {
                    "; the running window stays open"
                } else {
                    ""
                }
            ),
            Ok(build) => {
                let changed = session
                    .running
                    .as_ref()
                    .is_none_or(|running| running.stamps != stamps(&build.executables));
                if !forced && !changed {
                    eprintln!("ara-dev live: no executable changed; the window keeps its state");
                } else if let Err(error) = session.relaunch(&build) {
                    eprintln!("ara-dev live: cannot relaunch {name}: {error:#}");
                }
            }
        }
        forced = session.wait(&triggers)?;
    }
}

/// Rust sources, manifests and embedded resources rebuild; the app's live CSS does not.
fn watch(root: &Path, app: App, send: Sender<Trigger>) -> Result<notify::RecommendedWatcher> {
    let workspace = root.to_owned();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        if event
            .paths
            .iter()
            .any(|path| rebuilds(&workspace, app, path))
        {
            let _ = send.send(Trigger::Save);
        }
    })?;
    watcher.watch(&root.join("crates"), notify::RecursiveMode::Recursive)?;
    for manifest in ["Cargo.toml", "Cargo.lock"] {
        watcher.watch(&root.join(manifest), notify::RecursiveMode::NonRecursive)?;
    }
    Ok(watcher)
}

fn rebuilds(root: &Path, app: App, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    match relative.extension().and_then(|e| e.to_str()) {
        Some("rs" | "toml" | "lock" | "svg" | "png" | "ttf") => true,
        Some("css") => !app.live_css().iter().any(|live| relative.starts_with(live)),
        _ => false,
    }
}

fn stamps(executables: &[PathBuf]) -> Vec<(Option<SystemTime>, u64)> {
    executables
        .iter()
        .map(|path| {
            let metadata = path.metadata().ok();
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());
            (modified, metadata.map_or(0, |m| m.len()))
        })
        .collect()
}

impl Session<'_> {
    fn build(&self) -> Result<Build> {
        let executables: Vec<_> = match self.app {
            App::Editor => {
                let pair = crate::editor::build_pair(self.root)?;
                // Staged together: the editor finds the worker and CLI beside itself.
                [pair.editor, pair.player]
                    .into_iter()
                    .chain(pair.siblings)
                    .collect()
            }
            App::Hub => {
                let directories = crate::build_paths::Directories::from_env(self.root)?;
                let _lease = crate::compile_cache::acquire(Some(
                    Instant::now() + Duration::from_secs(2400),
                ))?;
                let mut cargo = Command::new("cargo");
                cargo
                    .current_dir(self.root)
                    .args(["build", "-p", "ara-hub", "-p", "ara"])
                    .args(["--bin", "ara-hub", "--bin", "ara"])
                    .arg("--message-format=json-render-diagnostics")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit());
                directories.apply(&mut cargo);
                crate::compile_cache::configure(&mut cargo);
                let output = cargo.spawn()?.wait_with_output()?;
                ensure!(output.status.success(), "Cargo failed");
                let binaries =
                    crate::binaries::artifacts(&String::from_utf8_lossy(&output.stdout))?;
                ["ara-hub", "ara"]
                    .into_iter()
                    .map(|name| {
                        binaries
                            .get(name)
                            .cloned()
                            .with_context(|| format!("Cargo did not report {name}"))
                    })
                    .collect::<Result<_>>()?
            }
        };
        let directory = executables[0]
            .parent()
            .context("artifact directory")?
            .to_owned();
        Ok(Build {
            directory,
            executables,
        })
    }

    /// Stage `build` as the next generation, then replace the running app with it.
    /// Relaunching stops the old process tree without saving (editor scenes included).
    fn relaunch(&mut self, build: &Build) -> Result<()> {
        self.generation += 1;
        let directory = self.live.join(format!("gen-{}", self.generation));
        let staged = stage(&directory, build)?;
        let mut command = self.command(build, &staged)?;
        if let Some(mut previous) = self.running.take() {
            previous.tree.stop();
            let _ = fs::remove_dir_all(&previous.directory);
        }
        let tree = crate::process::Tree::spawn(&mut command)
            .with_context(|| format!("start {}", staged[0].display()))?;
        eprintln!(
            "ara-dev live: started {} generation {} (pid {}); CSS/theme saves apply live, Rust saves rebuild, Enter relaunches",
            self.app.name(),
            self.generation,
            tree.id()
        );
        self.running = Some(Running {
            tree,
            stamps: stamps(&build.executables),
            directory,
        });
        Ok(())
    }

    /// The Hub keeps one scratch profile under `live` across relaunches (registered
    /// projects and preferences survive); the editor uses ara-dev's editor environment.
    /// Live apps run without Vulkan validation layers (25–40% of a dev frame, which
    /// hides how smooth resizing and scrolling really are); set
    /// `ARA_NO_GPU_VALIDATION=` (empty) before `ara-dev live` to keep them on.
    fn command(&self, build: &Build, staged: &[PathBuf]) -> Result<Command> {
        let mut command = self.app_command(build, staged)?;
        if std::env::var_os("ARA_NO_GPU_VALIDATION").is_none() {
            command.env("ARA_NO_GPU_VALIDATION", "1");
        }
        Ok(command)
    }
    fn app_command(&self, build: &Build, staged: &[PathBuf]) -> Result<Command> {
        match self.app {
            App::Editor => crate::editor::launch(
                self.root,
                &staged[0],
                &staged[1],
                &build.directory,
                self.args.clone(),
            ),
            App::Hub => {
                let config = self.live.join("config");
                fs::create_dir_all(&config)?;
                let mut command = Command::new(&staged[0]);
                command
                    .current_dir(self.root)
                    .arg("--config-dir")
                    .arg(&config)
                    .args(&self.args)
                    .env("ARA_CONFIG_DIR", &config);
                Ok(command)
            }
        }
    }

    /// Block until a save or Enter, then let a burst of saves settle; true when Enter
    /// asked for a relaunch. Reports an app that exits by itself.
    fn wait(&mut self, triggers: &Receiver<Trigger>) -> Result<bool> {
        let mut forced = loop {
            match triggers.recv_timeout(Duration::from_millis(500)) {
                Ok(trigger) => break trigger == Trigger::Enter,
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(running) = &mut self.running
                        && let Ok(Some(status)) = running.tree.try_wait()
                    {
                        eprintln!(
                            "ara-dev live: {} exited ({status}); save a file or press Enter to relaunch",
                            self.app.name()
                        );
                        self.running = None;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => bail!("the source watcher stopped"),
            }
        };
        loop {
            match triggers.recv_timeout(Duration::from_millis(300)) {
                Ok(trigger) => forced |= trigger == Trigger::Enter,
                Err(RecvTimeoutError::Timeout) => break Ok(forced),
                Err(RecvTimeoutError::Disconnected) => bail!("the source watcher stopped"),
            }
        }
    }
}

/// Copy the executables and the DLLs beside them into `directory`; the app runs that
/// copy, so its open files never block the next link.
fn stage(directory: &Path, build: &Build) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(directory)?;
    for entry in fs::read_dir(&build.directory)?.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
            && path.is_file()
        {
            fs::copy(&path, directory.join(entry.file_name()))?;
        }
    }
    build
        .executables
        .iter()
        .map(|executable| {
            let staged = directory.join(executable.file_name().context("executable name")?);
            fs::copy(executable, &staged)
                .with_context(|| format!("stage {}", executable.display()))?;
            Ok(staged)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_resources_rebuild_but_live_css_does_not() {
        let root = Path::new("/w");
        let rebuilds = |app, path: &str| rebuilds(root, app, &root.join(path));
        assert!(rebuilds(App::Hub, "crates/ara-hub/src/view/mod.rs"));
        assert!(rebuilds(App::Hub, "Cargo.lock"));
        assert!(!rebuilds(App::Hub, "crates/ara-hub/src/view/hub.css"));
        assert!(rebuilds(App::Hub, "crates/ara-editor/src/studio/mod.rs"));
        assert!(!rebuilds(
            App::Editor,
            "crates/ara-ui/src/dialogs/dialogs.css"
        ));
        assert!(rebuilds(
            App::Editor,
            "crates/ara-editor/src/open_project/open_project.css"
        ));
        assert!(!rebuilds(App::Editor, "crates/ara-editor/AGENTS.md"));
        assert!(!super::rebuilds(
            root,
            App::Editor,
            Path::new("/elsewhere/main.rs")
        ));
    }
}
