//! Keep the native editor and its pooled player in one Cargo feature/profile build.
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};

struct Plan {
    build: Vec<OsString>,
    launch: bool,
    arguments: Vec<OsString>,
}

fn value<'a>(args: &'a [OsString], index: usize, long: &str, short: &str) -> Option<&'a str> {
    let arg = args[index].to_str()?;
    if arg == long || (!short.is_empty() && arg == short) {
        return args.get(index + 1)?.to_str();
    }
    arg.strip_prefix(&format!("{long}=")).or_else(|| {
        (!short.is_empty())
            .then(|| arg.strip_prefix(short))
            .flatten()
    })
}

fn package(spec: &str, name: &str) -> bool {
    spec == name
        || spec
            .strip_prefix(name)
            .is_some_and(|suffix| suffix.starts_with('@'))
}

fn plan(args: &[OsString], workspace_editor: bool) -> Result<Option<Plan>> {
    let split = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    let cargo = &args[..split];
    let packages: Vec<_> = (0..cargo.len())
        .filter_map(|i| value(cargo, i, "--package", "-p"))
        .collect();
    let bins: Vec<_> = (0..cargo.len())
        .filter_map(|i| value(cargo, i, "--bin", ""))
        .collect();
    let workspace = cargo
        .iter()
        .any(|arg| arg == "--workspace" || arg == "--all");
    let excluded: Vec<_> = (0..cargo.len())
        .filter_map(|i| value(cargo, i, "--exclude", ""))
        .collect();
    let selected = packages.iter().any(|name| package(name, "ara-editor"))
        || (workspace_editor && packages.is_empty() && !excluded.contains(&"ara-editor"));
    if !selected && !bins.contains(&"ara-editor") {
        return Ok(None);
    }
    let Some(verb) = cargo.first().and_then(|arg| arg.to_str()) else {
        return Ok(None);
    };
    if !matches!(verb, "build" | "b" | "run" | "r") {
        ensure!(
            !cargo.iter().any(|arg| arg == "build" || arg == "run"),
            "editor companion flow requires the Cargo verb first: ara-dev cargo -- run -p ara-editor [Cargo options] -- [editor arguments]"
        );
        return Ok(None);
    }
    // Type checks and explicit library/example builds do not launch an editor.
    if bins.is_empty()
        && !cargo
            .iter()
            .any(|arg| arg == "--bins" || arg == "--all-targets")
        && cargo.iter().any(|arg| {
            arg == "--lib"
                || arg == "--examples"
                || arg == "--example"
                || arg.to_string_lossy().starts_with("--example=")
        })
    {
        return Ok(None);
    }
    if !bins.is_empty() && !bins.contains(&"ara-editor") {
        return Ok(None);
    }
    let launch = matches!(verb, "run" | "r");
    ensure!(
        !launch || packages.iter().any(|name| package(name, "ara-editor")),
        "select the editor explicitly so companion feature roots are preserved: ara-dev cargo -- run -p ara-editor [Cargo options] -- [editor arguments]"
    );
    ensure!(
        !excluded.contains(&"ara-player"),
        "cannot exclude the editor's companion ara-player; remove --exclude ara-player and build both together"
    );
    ensure!(
        !(0..cargo.len()).any(|i| value(cargo, i, "--message-format", "").is_some()),
        "editor companion builds collect Cargo JSON artifacts internally; omit --message-format (logs are under target/ara-dev/editor-*)"
    );
    let mut build = cargo.to_vec();
    build[0] = "build".into();
    if !packages.is_empty()
        && !workspace
        && !packages.iter().any(|name| package(name, "ara-player"))
    {
        build.extend(["--package".into(), "ara-player".into()]);
    }
    if bins.is_empty() {
        build.push("--bins".into());
    } else if !bins.contains(&"ara-player") {
        build.extend(["--bin".into(), "ara-player".into()]);
    }
    build.push("--message-format=json-render-diagnostics".into());
    Ok(Some(Plan {
        build,
        launch,
        arguments: args.get(split + 1..).unwrap_or_default().to_vec(),
    }))
}

/// Return None for ordinary focused Cargo work. Never run Cargo a second time
/// with only the editor root: that would discard the paired feature union.
pub(crate) fn cargo(root: &Path, args: &[OsString]) -> Result<Option<bool>> {
    let Some(plan) = plan(args, root.join("crates/ara-editor/Cargo.toml").is_file())? else {
        return Ok(None);
    };
    let directories = crate::build_paths::Directories::from_env(root)?;
    crate::build_paths::validate_output(root, &root.join("target"))?;
    for i in 0..plan.build.len() {
        if let Some(path) = value(&plan.build, i, "--target-dir", "") {
            crate::build_paths::validate_output(root, Path::new(path))?;
        }
    }
    let lease = crate::compile_cache::acquire(Some(Instant::now() + Duration::from_secs(2400)))?;
    let directory = root.join(format!(
        "target/ara-dev/editor-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    ));
    fs::create_dir_all(&directory)?;
    let artifacts = directory.join("build.jsonl");
    let mut build = Command::new("cargo");
    build
        .current_dir(root)
        .args(&plan.build)
        .stdout(File::create(&artifacts)?);
    directories.apply(&mut build);
    crate::compile_cache::configure(&mut build);
    eprintln!(
        "Building editor + matching player with one feature/profile resolution: {:?}",
        plan.build
    );
    ensure!(
        crate::process::run(&mut build, None)?.success(),
        "editor/player build failed; editor was not launched and an existing player was not reused (Cargo log: {})",
        artifacts.display()
    );
    let binaries = crate::binaries::artifacts(&fs::read_to_string(&artifacts)?)?;
    let editor = binaries.get("ara-editor");
    // A workspace's default members may deliberately omit the editor.
    if editor.is_none() && !plan.launch {
        return Ok(Some(true));
    }
    let editor = editor
        .context("Cargo did not produce ara-editor; select -p ara-editor --bin ara-editor")?;
    let player = binaries.get("ara-player").context(
        "missing matching companion ara-player; select both -p ara-editor -p ara-player with the same profile, target and features (do not exclude its bin)"
    )?;
    ensure!(
        editor.is_file() && player.is_file(),
        "Cargo reported missing editor/player artifacts; rebuild the pair instead of using an old executable"
    );
    ensure!(
        editor.parent() == player.parent(),
        "editor/player artifacts have different target/profile directories: {} and {}",
        editor.display(),
        player.display()
    );
    if let Some(override_path) = std::env::var_os("ARA_PLAYER").filter(|path| !path.is_empty()) {
        let override_path = root.join(override_path);
        ensure!(
            override_path.canonicalize().ok() == Some(player.canonicalize()?),
            "stale or unverified ARA_PLAYER override {} differs from matching companion {}; unset ARA_PLAYER and rerun this ara-dev command",
            override_path.display(),
            player.display()
        );
    }
    for ray_tracing in [false, true] {
        let log = directory.join(format!("player-ray-tracing-{ray_tracing}.log"));
        let mut probe = Command::new(player);
        probe
            .current_dir(root)
            .args([format!("--ray-tracing={ray_tracing}"), "--help".into()])
            .env("ARA_CONFIG_DIR", directory.join("probe-config"))
            .env("ARA_CRASH_DIR", directory.join("probe-crashes"))
            .stdin(Stdio::null())
            .stdout(File::create(&log)?)
            .stderr(File::create(log.with_extension("err"))?);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            probe.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let outcome = crate::process::run(
            &mut probe,
            Some(Instant::now() + Duration::from_secs(15)),
        )
        .with_context(|| {
            format!(
                "cannot verify companion {} on this host; build and run the matching native target",
                player.display()
            )
        })?;
        ensure!(
            outcome.success() && fs::read_to_string(&log)?.contains("--ray-tracing"),
            "stale or incompatible companion {} rejects or times out on --ray-tracing={ray_tracing} --help. The editor was not launched. Remove the stale companion executable and rerun this same ara-dev cargo build/run command to rebuild both with matching roots/profile/target. Probe: {}\n{}",
            player.display(),
            log.display(),
            fs::read_to_string(log.with_extension("err"))?
        );
    }
    eprintln!("Matching companion verified: {}", player.display());
    // Final executables are worktree-local; a live editor need not hold the
    // shared intermediate compilation lane while other worktrees build.
    drop(lease);
    if !plan.launch {
        return Ok(Some(true));
    }
    let mut command = Command::new(editor);
    command
        .current_dir(root)
        .args(plan.arguments)
        .env("ARA_PLAYER", player);
    for (variable, fallback) in [
        ("ARA_CONFIG_DIR", "target/ara-config"),
        ("ARA_CRASH_DIR", "target/ara-crashes"),
    ] {
        if std::env::var_os(variable).is_none_or(|value| value.is_empty()) {
            command.env(variable, root.join(fallback));
        }
    }
    let parent = editor.parent().context("editor artifact directory")?;
    let mut paths = vec![parent.to_owned(), parent.join("deps")];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env("PATH", std::env::join_paths(paths)?);
    Ok(Some(crate::process::run(&mut command, None)?.success()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn paired_build_preserves_all_roots_features_target_and_profile() {
        let input = args(&[
            "run",
            "-p",
            "ara-editor",
            "-p",
            "owner",
            "--profile",
            "perf",
            "--target",
            "x86_64-pc-windows-msvc",
            "--features",
            "owner/extra",
            "--bin",
            "ara-editor",
            "--",
            "project with spaces",
            "--frames",
            "3",
        ]);
        let plan = plan(&input, true).unwrap().unwrap();
        let end = input.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(&plan.build[1..end], &input[1..end]);
        assert!(
            plan.build
                .windows(2)
                .any(|pair| pair == args(&["--package", "ara-player"]))
        );
        assert!(
            plan.build
                .windows(2)
                .any(|pair| pair == args(&["--bin", "ara-player"]))
        );
        assert_eq!(
            plan.arguments,
            args(&["project with spaces", "--frames", "3"])
        );
        assert!(plan.launch);
    }

    #[test]
    fn unrelated_checks_and_library_builds_stay_focused() {
        for input in [
            args(&["clippy", "-p", "ara-editor"]),
            args(&["test", "-p", "ara-editor"]),
            args(&["build", "-p", "ara-editor", "--lib"]),
            args(&["run", "-p", "ara-dev"]),
        ] {
            assert!(plan(&input, true).unwrap().is_none());
        }
        assert!(
            plan(
                &args(&["build", "--workspace", "--exclude", "ara-player"]),
                true
            )
            .is_err()
        );
        assert!(plan(&args(&["run", "--bin", "ara-editor"]), true).is_err());
        let plan = plan(
            &args(&["b", "--package=ara-editor@0.1.0", "--profile=perf"]),
            false,
        )
        .unwrap()
        .unwrap();
        assert!(!plan.launch);
        assert!(plan.build.contains(&OsString::from("--profile=perf")));
    }
}
