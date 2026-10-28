//! Laptop job policy. The daemon never holds a sleep assertion while idle.
use anyhow::{Result, ensure};

pub(super) fn require_ac(text: &str) -> Result<()> {
    ensure!(
        text.contains("Now drawing from 'AC Power'"),
        "ARA_LAB_MAC_BATTERY: Mac lab job refused: connect the charger (pmset reports battery power); no build or player job started"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) struct Job(crate::process::Tree);
#[cfg(not(target_os = "macos"))]
pub(super) struct Job;

impl Job {
    pub fn start() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            use std::process::{Command, Stdio};
            let state = Command::new("/usr/bin/pmset")
                .args(["-g", "batt"])
                .output()?;
            ensure!(state.status.success(), "cannot verify Mac charger state");
            require_ac(&String::from_utf8(state.stdout)?)?;
            let mut cmd = Command::new("/usr/bin/caffeinate");
            cmd.args(["-dimsu", "-w", &std::process::id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            Ok(Self(crate::process::Tree::spawn(&mut cmd)?))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Self)
        }
    }
}
#[cfg(target_os = "macos")]
impl Drop for Job {
    fn drop(&mut self) {
        let _ = self.0.try_wait();
    }
}

pub(super) fn build_disk(target: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/df")
            .arg("-Pk")
            .arg(target)
            .output()?;
        ensure!(output.status.success(), "cannot verify Mac build disk");
        let text = String::from_utf8(output.stdout)?;
        let free = text
            .lines()
            .last()
            .and_then(|l| l.split_whitespace().nth(3))
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("invalid Mac disk sample"))?;
        let default = if target.join("perf/ara-player").is_file() {
            2
        } else {
            15
        };
        let minimum = std::env::var("ARA_LAB_MIN_DISK_GIB")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(default);
        ensure!(
            free >= minimum * 1024 * 1024,
            "Mac lab needs {minimum} GiB free for this build; {:.2} GiB available at {}",
            free as f64 / (1024. * 1024.),
            target.display()
        );
        let competing = std::process::Command::new("/usr/bin/pgrep")
            .args(["-x", "cargo"])
            .output()?;
        ensure!(
            !competing.status.success(),
            "another Mac Cargo build is running; the lab permits one build at a time"
        );
    }
    #[cfg(not(target_os = "macos"))]
    let _ = target;
    Ok(())
}

/// Abort our build before disk exhaustion can affect the shared laptop.
pub(super) fn running_disk(target: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/df")
            .arg("-Pk")
            .arg(target)
            .output()?;
        ensure!(output.status.success(), "cannot monitor Mac build disk");
        let text = String::from_utf8(output.stdout)?;
        let free = text
            .lines()
            .last()
            .and_then(|s| s.split_whitespace().nth(3))
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("invalid Mac disk sample"))?;
        ensure!(
            free >= 1024 * 1024,
            "Mac build stopped before disk exhaustion: {:.2} GiB free",
            free as f64 / (1024. * 1024.)
        );
    }
    #[cfg(not(target_os = "macos"))]
    let _ = target;
    Ok(())
}

/// Windows scene packs share assets, while their gameplay images must be native.
#[cfg(target_os = "macos")]
pub(super) fn scripts(
    root: &std::path::Path,
    base: &std::path::Path,
    pack: &std::path::Path,
    scene: &str,
) -> Result<()> {
    if scene == "showcase" {
        return Ok(());
    }
    use anyhow::Context;
    let signature = crate::git::text(
        root,
        ["rev-parse", "HEAD:samples/outpost/scripts", "HEAD:dotnet"],
    )?;
    let marker = pack.join(".lab-native-scripts");
    if std::fs::read_to_string(&marker).is_ok_and(|s| s == signature)
        && pack.join("game/Outpost.dylib").is_file()
    {
        return Ok(());
    }
    build_disk(&super::build::default_directory(root))?;
    let _power = Job::start()?;
    let binaries: crate::binaries::Binaries = serde_json::from_value(
        super::session::read(&base.join("binaries.json"))
            .context("Mac build has not produced ara-export yet")?,
    )?;
    let exporter = binaries
        .get("ara-export")
        .context("Mac build did not produce ara-export")?;
    let job = base.join("native-scripts").join(super::session::id());
    std::fs::create_dir_all(&job)?;
    let mut command = std::process::Command::new("/usr/bin/nice");
    command
        .args(["-n", "10"])
        .arg(exporter)
        .args(["scripts", "--project"])
        .arg(root.join("samples/outpost"))
        .args(["--assembly", "Outpost", "--rid", "osx-arm64", "--output"])
        .arg(pack.join("game"))
        .arg("--job")
        .arg(&job)
        .env("CARGO_BUILD_JOBS", "1")
        .stdout(std::fs::File::create(job.join("scripts.log"))?)
        .stderr(std::fs::File::create(job.join("scripts.stderr.log"))?);
    anyhow::ensure!(
        crate::process::run(
            &mut command,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(600))
        )?
        .success(),
        "native Mac gameplay image failed; {}",
        job.display()
    );
    std::fs::write(marker, signature)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn battery_and_unknown_states_refuse_even_when_fully_charged() {
        assert!(require_ac("Now drawing from 'Battery Power'\n100%; discharging").is_err());
        assert!(require_ac("unknown").is_err());
        assert!(require_ac("Now drawing from 'AC Power'\n82%; charging").is_ok());
    }
}
