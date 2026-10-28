//! Lab strategy adapter. The lab owns sessions, timing admission and transport.
mod build;
mod cache;
mod fallback;
mod launcher;
pub(super) mod source;
use crate::{
    binaries::Binaries,
    lab::build::{ReloadStrategy, ReloadTarget, WarmRestart},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

pub(super) fn forward() -> Option<ExitCode> {
    launcher::forward()
}

pub(super) fn run(root: &Path, dx: PathBuf, plan: bool) -> Result<bool> {
    let target = super::output_root(root);
    if plan {
        println!(
            "lab Rust strategy=hotpatch when compatible; target={}; resident players=1",
            target.display()
        );
        return Ok(true);
    }
    cache::restore_template(root)?;
    if !cfg!(windows) {
        eprintln!(
            "ARA7600: native lab strategy is currently Windows-verified; using rebuild + warm restart"
        );
        return crate::lab::run_with_strategy(root, &target, 1, Box::new(fallback::Warm));
    }
    let available = super::available(&dx).and_then(|_| {
        let capability = super::tool::command(&dx)
            .arg("--ara-native-rust-patch-version")
            .output()?;
        ensure!(
            capability.stdout.trim_ascii() == b"4",
            "ARA7600: lab admission requires native dx capability v4"
        );
        Ok(())
    });
    if let Err(error) = available {
        eprintln!("{error:#}; lab uses rebuild + warm restart");
        return crate::lab::run_with_strategy(root, &target, 1, Box::new(fallback::Warm));
    }
    eprintln!(
        "Experimental lab Rust hotpatch: one shared player; body edits stage before publication. Ordinary ara-dev lab retains its multi-track warm-restart strategy."
    );
    let result = crate::lab::run_with_strategy(
        root,
        &target,
        1,
        Box::new(Strategy {
            root: root.to_owned(),
            dx,
            target: target.to_owned(),
            sources: source::Sources::default(),
        }),
    );
    let restored = cache::restore_template(root);
    result.and_then(|value| restored.map(|()| value))
}

struct Strategy {
    root: PathBuf,
    dx: PathBuf,
    target: PathBuf,
    sources: source::Sources,
}
impl ReloadStrategy for Strategy {
    fn name(&self) -> &'static str {
        "hotpatch_or_warm_restart"
    }
    fn command(&self, root: &Path, target: &Path) -> Command {
        let mut command =
            super::tool::command(std::env::current_exe().unwrap_or_else(|_| "ara-dev".into()));
        command.current_dir(root).args(["hotpatch", "--"]);
        match self.sources.prepare(root, &self.dx, target) {
            Ok(source) => {
                command.arg("lab-build").arg(target).arg(source);
            }
            Err(error) => {
                command.arg("lab-error").arg(format!("{error:#}"));
            }
        }
        command
    }
    fn stage(&self, directory: &Path, cache: &Path) -> Result<Binaries> {
        let output = fs::read_to_string(directory.join("cargo.jsonl"))?;
        if let Some(batch) = output
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find_map(|v| v["ara_hotpatch_batch"].as_str().map(PathBuf::from))
        {
            let mut binaries: Binaries =
                serde_json::from_slice(&fs::read(cache.join("binaries.json"))?)?;
            binaries.insert("ara-hotpatch-batch".into(), batch);
            return Ok(binaries);
        }
        let mut binaries = WarmRestart.stage(directory, cache)?;
        let player = binaries
            .get_mut("ara-player")
            .context("matching lab player")?;
        let template = player.to_owned();
        // Scene export needs matching symbols beside its immutable normal player.
        // WarmRestart stages the executable/DLL, while Cargo leaves the PDB behind.
        if cfg!(windows) {
            let artifacts = crate::binaries::artifacts(&output)?;
            let built = artifacts
                .get("ara-player")
                .context("normal player artifact")?;
            fs::copy(
                built.with_file_name("ara_player.pdb"),
                template.with_file_name("ara_player.pdb"),
            )
            .context("preserve matching normal lab template symbols")?;
        }
        let source = self.sources.prepare(&self.root, &self.dx, &self.target)?;
        *player = launcher::stage(player, &source)?;
        // Freezing a new lab scene must export the normal player, never our launcher.
        binaries.insert("ara-player-template".into(), template);
        Ok(binaries)
    }
    fn publish(&self, binaries: &Binaries, targets: &[ReloadTarget]) -> Result<bool> {
        let Some(batch) = binaries.get("ara-hotpatch-batch") else {
            return Ok(true);
        };
        let row: Value = serde_json::from_slice(&fs::read(batch)?)?;
        ensure!(
            targets.len() == 1 && row["owner_pid"].as_u64() == Some(u64::from(targets[0].pid)),
            "hotpatch target PID changed; last good retained"
        );
        let control = PathBuf::from(row["control"].as_str().context("batch control")?);
        ensure!(
            targets[0].control == control,
            "hotpatch target control directory changed"
        );
        let contract = ara_hotpatch::Contract::read(Path::new(
            row["contract"].as_str().context("batch baseline")?,
        ))?;
        let revision = row["revision"].as_str().context("batch source revision")?;
        ensure!(
            contract.revision()? == revision,
            "newer Rust sources superseded the staged patch"
        );
        let directory = control.join("rust");
        fs::write(directory.join("publish"), revision)?;
        let began = Instant::now();
        loop {
            let applied = read(&directory.join("applied.json"));
            if applied["revision"] == revision && applied["pid"] == row["pid"] {
                break;
            }
            ensure!(
                // Library loading/relocation stays at the frame boundary. Shared
                // disk stalls measured above five seconds must keep lab admission
                // until the actual apply acknowledgement, not report success early.
                began.elapsed() < Duration::from_secs(60),
                "hotpatch frame-boundary acknowledgement timed out; last good retained"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        eprintln!(
            "{}",
            json!({"kind":"rust_patch_applied","pid":row["pid"],"owner_pid":row["owner_pid"],"revision":revision})
        );
        Ok(false)
    }
}

fn read(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

pub(super) fn build(root: &Path, target: &Path, source: &Path) -> Result<bool> {
    build::run(root, target, source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_binds_pid_control_and_current_sources_without_restarting() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        fs::create_dir_all(root.join("crates/example")).unwrap();
        let source = root.join("crates/example/lib.rs");
        fs::write(&source, "pub fn body() { let _a = 1; }").unwrap();
        let contract = ara_hotpatch::Contract::capture(root).unwrap();
        let revision = contract.revision().unwrap();
        let baseline = root.join("baseline.json");
        contract.write(&baseline).unwrap();
        let control = root.join("control");
        fs::create_dir_all(control.join("rust")).unwrap();
        fs::write(
            control.join("rust/applied.json"),
            serde_json::to_vec(&json!({"revision":revision,"pid":13})).unwrap(),
        )
        .unwrap();
        let batch = root.join("batch.json");
        fs::write(&batch, serde_json::to_vec(&json!({"revision":revision,"pid":13,"owner_pid":12,"contract":baseline,"control":control})).unwrap()).unwrap();
        let binaries = Binaries::from([("ara-hotpatch-batch".into(), batch)]);
        let strategy = Strategy {
            root: root.to_owned(),
            dx: root.join("dx"),
            target: root.to_owned(),
            sources: source::Sources::default(),
        };
        assert!(
            strategy
                .publish(
                    &binaries,
                    &[ReloadTarget {
                        pid: 99,
                        control: control.to_owned()
                    }]
                )
                .is_err()
        );
        assert!(
            strategy
                .publish(
                    &binaries,
                    &[ReloadTarget {
                        pid: 12,
                        control: root.join("other")
                    }]
                )
                .is_err()
        );
        let target = [ReloadTarget {
            pid: 12,
            control: control.to_owned(),
        }];
        assert!(!strategy.publish(&binaries, &target).unwrap());
        fs::remove_file(control.join("rust/publish")).unwrap();
        fs::write(&source, "pub fn body() { let _a = 2; }").unwrap();
        assert!(strategy.publish(&binaries, &target).is_err());
        assert!(!control.join("rust/publish").exists());
    }
}
