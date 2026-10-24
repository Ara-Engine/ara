//! The real setup executable with tiny fixture payloads, driven only through its
//! console modes (`--silent`, `--list`, `--help`): no window ever opens. Every
//! destination is a temp directory, and registration is a fixture `install.ps1`.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]
use std::{
    fs,
    io::Write,
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

/// The setup under test; `ARA_SETUP_STUB` checks another build (e.g. release).
fn stub() -> PathBuf {
    std::env::var_os("ARA_SETUP_STUB").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_ara-setup")),
        PathBuf::from,
    )
}
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Records its switches, or fails when the package contains `fail.txt`.
const SCRIPT: &str = r#"param([switch]$RegisterOnly, [switch]$Silent, [switch]$NoSystemIntegration)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'fail.txt')) { throw 'fixture registration failure' }
Set-Content -LiteralPath (Join-Path $PSScriptRoot 'registered.txt') -Value "register=$RegisterOnly silent=$Silent isolated=$NoSystemIntegration"
Write-Host 'fixture registered'
"#;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::create_dir(root.path().join("programs")).unwrap();
        Self { root }
    }
    fn destination(&self) -> PathBuf {
        self.root.path().join("programs/Ara 0.0.1-alpha")
    }
    /// An engine setup whose payload holds `files` besides the manifest and script.
    fn engine(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let manifest = r#"{"format":"ara.engine/1","version":"0.0.1-alpha"}"#;
        for (path, text) in [("engine.json", manifest), ("install.ps1", SCRIPT)]
            .into_iter()
            .chain(files.iter().copied())
        {
            zip.start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        let archive = zip.finish().unwrap().into_inner();
        let exe = self.root.path().join(format!("{name}-setup.exe"));
        fs::write(
            &exe,
            ara_export_installer::append(&fs::read(stub()).unwrap(), &archive).unwrap(),
        )
        .unwrap();
        exe
    }
    fn run(&self, exe: &Path, args: &[&str]) -> Output {
        Command::new(exe)
            .args(args)
            .env("ARA_CONFIG_DIR", self.root.path().join("config"))
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .unwrap()
    }
    fn install(&self, exe: &Path, extra: &[&str]) -> Output {
        let destination = self.destination();
        let mut args = vec![
            "--silent",
            "--no-system-integration",
            "--destination",
            destination.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        self.run(exe, &args)
    }
    /// Stage and backup siblings left in `programs/`.
    fn leftovers(&self) -> Vec<String> {
        fs::read_dir(self.root.path().join("programs"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".ara-"))
            .collect()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_fresh_silent_install_registers_and_reports_on_the_console() {
    let fixture = Fixture::new();
    let exe = fixture.engine("v1", &[("bin/old.txt", "old")]);
    let output = fixture.install(&exe, &[]);
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{stdout}{}", text(&output.stderr));
    assert!(stdout.contains("fixture registered"), "{stdout}");
    assert!(stdout.contains("Ara 0.0.1-alpha installed at"), "{stdout}");
    let destination = fixture.destination();
    assert_eq!(
        fs::read_to_string(destination.join("bin/old.txt")).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(destination.join("registered.txt"))
            .unwrap()
            .trim(),
        "register=True silent=True isolated=True"
    );
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn the_same_version_needs_replace_then_swaps_without_leftovers() {
    let fixture = Fixture::new();
    let old = fixture.engine("v1", &[("bin/old.txt", "old")]);
    let new = fixture.engine("v2", &[("bin/new.txt", "new")]);
    assert!(fixture.install(&old, &[]).status.success());
    let refused = fixture.install(&new, &[]);
    let stderr = text(&refused.stderr);
    assert!(!refused.status.success());
    assert!(stderr.contains("ARA-EXP-INSTALL-004"), "{stderr}");
    assert!(stderr.contains("--replace"), "{stderr}");
    assert!(fixture.destination().join("bin/old.txt").is_file());

    let replaced = fixture.install(&new, &["--replace"]);
    assert!(replaced.status.success(), "{}", text(&replaced.stderr));
    let destination = fixture.destination();
    assert!(destination.join("bin/new.txt").is_file());
    assert!(!destination.join("bin/old.txt").exists());
    assert!(destination.join("registered.txt").is_file());
    assert!(fixture.leftovers().is_empty(), "{:?}", fixture.leftovers());
}

#[test]
fn a_folder_that_is_not_an_ara_install_is_never_touched() {
    let fixture = Fixture::new();
    let exe = fixture.engine("v1", &[]);
    let destination = fixture.destination();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), "mine").unwrap();
    let plain = fixture.install(&exe, &[]);
    assert!(text(&plain.stderr).contains("ARA-EXP-INSTALL-004"));
    let replace = fixture.install(&exe, &["--replace"]);
    let stderr = text(&replace.stderr);
    assert!(!replace.status.success());
    assert!(stderr.contains("ARA-EXP-INSTALL-006"), "{stderr}");
    let names: Vec<_> = fs::read_dir(&destination).unwrap().collect();
    assert_eq!(names.len(), 1);
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).unwrap(),
        "mine"
    );
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn failed_registration_keeps_the_files_and_reports_its_output() {
    let fixture = Fixture::new();
    let exe = fixture.engine("broken", &[("fail.txt", "")]);
    let output = fixture.install(&exe, &[]);
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("ARA-EXP-INSTALL-010"), "{stderr}");
    assert!(stderr.contains("fixture registration failure"), "{stderr}");
    assert!(fixture.destination().join("engine.json").is_file());
}

#[test]
fn replace_refuses_while_a_program_runs_from_the_folder() {
    let fixture = Fixture::new();
    let old = fixture.engine("v1", &[("bin/old.txt", "old")]);
    let new = fixture.engine("v2", &[("bin/new.txt", "new")]);
    assert!(fixture.install(&old, &[]).status.success());
    let editor = fixture.destination().join("ara-editor.exe");
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    fs::copy(system.join("System32/PING.EXE"), &editor).unwrap();
    let mut running = Command::new(&editor)
        .args(["-n", "60", "127.0.0.1"])
        .stdout(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let output = fixture.install(&new, &["--replace"]);
    let _ = running.kill();
    let _ = running.wait();
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("ARA-EXP-INSTALL-007"), "{stderr}");
    assert!(stderr.contains("Ara Editor (ara-editor.exe"), "{stderr}");
    assert!(fixture.destination().join("bin/old.txt").is_file());
    assert!(fixture.leftovers().is_empty(), "{:?}", fixture.leftovers());
}

#[test]
fn a_locked_folder_rolls_back_to_the_previous_install() {
    let fixture = Fixture::new();
    let old = fixture.engine("v1", &[("bin/old.txt", "old")]);
    let new = fixture.engine("v2", &[("bin/new.txt", "new")]);
    assert!(fixture.install(&old, &[]).status.success());
    // Held without sharing, as an editor or scanner holding a file would.
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(fixture.destination().join("bin/old.txt"))
        .unwrap();
    let output = fixture.install(&new, &["--replace"]);
    drop(lock);
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("ARA-EXP-INSTALL-008"), "{stderr}");
    assert!(stderr.contains("nothing was changed"), "{stderr}");
    assert!(fixture.destination().join("bin/old.txt").is_file());
    assert!(!fixture.destination().join("bin/new.txt").exists());
    assert!(fixture.leftovers().is_empty(), "{:?}", fixture.leftovers());
}

#[test]
fn console_modes_print_and_set_exit_codes() {
    let fixture = Fixture::new();
    let exe = fixture.engine("v1", &[("bin/old.txt", "old")]);
    let list = fixture.run(&exe, &["--list"]);
    assert!(list.status.success());
    assert_eq!(
        text(&list.stdout).lines().collect::<Vec<_>>(),
        ["engine.json", "install.ps1", "bin/old.txt"]
    );
    let help = fixture.run(&exe, &["--help"]);
    assert!(help.status.success());
    assert!(text(&help.stdout).contains("--replace"));
    let version = fixture.run(&exe, &["--version"]);
    assert_eq!(text(&version.stdout).trim(), "ara-setup 0.0.1-alpha");
    let wrong = fixture.run(&exe, &["--bogus"]);
    assert_eq!(wrong.status.code(), Some(2));
    assert!(text(&wrong.stderr).contains("unknown argument --bogus"));

    let mut corrupt = fs::read(&exe).unwrap();
    let middle = corrupt.len() - 200;
    corrupt[middle] ^= 0xff;
    let broken = fixture.root.path().join("corrupt-setup.exe");
    fs::write(&broken, corrupt).unwrap();
    let output = fixture.install(&broken, &[]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("ARA-EXP-INSTALL-001"));
    assert!(!fixture.destination().exists());
}

/// The window's side of the transaction, scripted: answers and recorded steps.
struct Scripted {
    replace: bool,
    asked: Vec<String>,
    steps: Vec<String>,
}

impl ara_setup::flow::Host for Scripted {
    fn plan(&mut self, plan: &ara_setup::flow::Plan) {
        self.steps.push(format!("plan {:?}", plan.kind));
    }
    fn progress(&mut self, step: ara_setup::flow::Step<'_>) {
        let name = format!("{step:?}");
        let name = name.split([' ', '{']).next().unwrap().to_owned();
        if self.steps.last() != Some(&name) {
            self.steps.push(name);
        }
    }
    fn confirm_replace(&mut self, _: &ara_setup::flow::Plan, installed: &str) -> bool {
        self.asked.push(installed.to_owned());
        self.replace
    }
    fn resolve(
        &mut self,
        _: &ara_setup::flow::Plan,
        _: &[ara_setup::processes::Blocker],
    ) -> ara_setup::flow::Resolution {
        ara_setup::flow::Resolution::Cancel
    }
}

#[test]
fn the_window_asks_before_replacing_and_cancel_changes_nothing() {
    use ara_setup::flow::{Outcome, run};
    let fixture = Fixture::new();
    let old = fixture.engine("v1", &[("bin/old.txt", "old")]);
    let new = fixture.engine("v2", &[("bin/new.txt", "new")]);
    assert!(fixture.install(&old, &[]).status.success());
    let args = ara_setup::args::Args {
        destination: Some(fixture.destination()),
        no_system_integration: true,
        ..Default::default()
    };
    let mut cancel = Scripted {
        replace: false,
        asked: Vec::new(),
        steps: Vec::new(),
    };
    assert_eq!(run(&new, &args, &mut cancel).unwrap(), Outcome::Cancelled);
    assert_eq!(cancel.asked, ["0.0.1-alpha"]);
    assert!(fixture.destination().join("bin/old.txt").is_file());
    assert!(fixture.leftovers().is_empty());

    let mut replace = Scripted {
        replace: true,
        asked: Vec::new(),
        steps: Vec::new(),
    };
    let Outcome::Installed(installed) = run(&new, &args, &mut replace).unwrap() else {
        panic!("replace was confirmed");
    };
    assert!(installed.replaced && installed.leftover.is_none());
    assert!(installed.output.contains("fixture registered"));
    assert_eq!(
        replace.steps,
        [
            "Verify",
            "plan Engine",
            "Extract",
            "Swap",
            "Cleanup",
            "Register"
        ]
    );
    assert!(fixture.destination().join("bin/new.txt").is_file());
    assert!(!fixture.destination().join("bin/old.txt").exists());
}
