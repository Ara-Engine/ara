//! SSH jobs use stdin scripts, exact argv and private logs; no persistent remote helper.
use super::Config;
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub(super) fn job(base: &Path) -> Result<PathBuf> {
    let path = base.join("mirror/jobs").join(super::super::session::id());
    std::fs::create_dir_all(&path)?;
    Ok(path)
}
pub(super) fn ssh(config: &Config, base: &Path, script: &str, seconds: u64) -> Result<String> {
    let path = job(base)?;
    let input = path.join("input.sh");
    std::fs::write(&input, script.replace('\r', ""))?;
    let stdout = path.join("stdout.log");
    let stderr = path.join("stderr.log");
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=2",
        &config.host,
        "/bin/sh -s",
    ])
    .stdin(std::fs::File::open(input)?)
    .stdout(std::fs::File::create(&stdout)?)
    .stderr(std::fs::File::create(&stderr)?);
    super::super::build::background(&mut cmd);
    let result = crate::process::run(
        &mut cmd,
        Some(Instant::now() + Duration::from_secs(seconds)),
    )?;
    ensure!(
        result.success(),
        "Mac SSH job failed: {}; evidence {}",
        std::fs::read_to_string(stderr).unwrap_or_default(),
        path.display()
    );
    Ok(std::fs::read_to_string(stdout)?)
}
pub(super) fn scp(config: &Config, base: &Path, source: &Path, destination: &str) -> Result<()> {
    let path = job(base)?;
    let stderr = path.join("scp.log");
    let mut cmd = Command::new("scp");
    cmd.args(["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(source)
        .arg(format!("{}:{destination}", config.host))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr)?);
    super::super::build::background(&mut cmd);
    ensure!(
        crate::process::run(&mut cmd, Some(Instant::now() + Duration::from_secs(300)))?.success(),
        "Mac transfer failed; {}",
        stderr.display()
    );
    Ok(())
}
pub(super) fn download(
    config: &Config,
    base: &Path,
    source: &str,
    destination: &Path,
) -> Result<()> {
    let path = job(base)?;
    let stderr = path.join("scp.log");
    let mut cmd = Command::new("scp");
    cmd.args(["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(format!("{}:{source}", config.host))
        .arg(destination)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr)?);
    super::super::build::background(&mut cmd);
    ensure!(
        crate::process::run(&mut cmd, Some(Instant::now() + Duration::from_secs(300)))?.success(),
        "Mac evidence transfer failed; {}",
        stderr.display()
    );
    Ok(())
}
pub(super) fn prelude(config: &Config) -> String {
    format!(
        "set -eu\nexport PATH=\"$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/share/dotnet:$PATH\"\nexport VULKAN_SDK=\"${{VULKAN_SDK:-$HOME/VulkanSDK/1.4.363.0/macOS}}\"\ncd {}\n",
        quote(&config.repo)
    )
}
pub(super) fn rpc(
    config: &Config,
    base: &Path,
    tool: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value> {
    let input = serde_json::to_string(&serde_json::json!({"tool":tool,"arguments":args}))?;
    let script = format!(
        "{}printf '%s\\n' {} | {} lab rpc\n",
        prelude(config),
        quote(&input),
        quote(&format!("{}/debug/ara-dev", config.build_dir))
    );
    let output = ssh(config, base, &script, 650)?;
    let value: serde_json::Value =
        serde_json::from_str(&output).context("Mac lab returned invalid JSON")?;
    if let Some(error) = value["error"].as_str() {
        anyhow::bail!("{error}");
    }
    Ok(value["result"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn posix_arguments_preserve_spaces_and_quotes() {
        assert_eq!(quote("a'b $c"), "'a'\\''b $c'");
    }
}
