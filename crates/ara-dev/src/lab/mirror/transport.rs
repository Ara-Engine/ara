//! SSH jobs use exact argv, stdin scripts or archives, and private logs.
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
    super::require_slot(base)?;
    let path = job(base)?;
    let input = path.join("input.sh");
    std::fs::write(&input, script.replace('\r', ""))?;
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
    ]);
    run_input(&mut cmd, &path, &input, seconds)
}

/// Upload a complete archive and enqueue its import in one SSH connection.
pub(super) fn stream(config: &Config, base: &Path, archive: &Path, script: &str) -> Result<String> {
    super::require_slot(base)?;
    let path = job(base)?;
    std::fs::write(path.join("input.sh"), script.replace('\r', ""))?;
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
    ])
    .arg(format!("/bin/sh -c {}", quote(script)));
    run_input(&mut cmd, &path, archive, 300)
}

fn run_input(cmd: &mut Command, path: &Path, input: &Path, seconds: u64) -> Result<String> {
    let started = Instant::now();
    let stdout = path.join("stdout.log");
    let stderr = path.join("stderr.log");
    cmd.stdin(std::fs::File::open(input)?)
        .stdout(std::fs::File::create(&stdout)?)
        .stderr(std::fs::File::create(&stderr)?);
    super::super::build::background(cmd);
    let result = crate::process::run(cmd, Some(Instant::now() + Duration::from_secs(seconds)))?;
    super::super::session::write(
        &path.join("timing.json"),
        &serde_json::json!({"seconds":started.elapsed().as_secs_f64(),"input_bytes":std::fs::metadata(input)?.len(),"success":result.success()}),
    )?;
    ensure!(
        result.success(),
        "Mac SSH job failed: {}; evidence {}",
        std::fs::read_to_string(stderr).unwrap_or_default(),
        path.display()
    );
    Ok(std::fs::read_to_string(stdout)?)
}
pub(super) fn download(
    config: &Config,
    base: &Path,
    source: &str,
    destination: &Path,
) -> Result<()> {
    super::require_slot(base)?;
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
    response(&output)
}

pub(super) fn response(output: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_str(output).context("Mac lab returned invalid JSON")?;
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
    #[test]
    fn archive_stdin_preserves_binary_bytes_without_a_shell_or_an_extra_process() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(super::super::super::session::id());
        std::fs::create_dir_all(&root)?;
        let input = root.join("input.tar");
        let received = root.join("received.tar");
        let bytes = (0_u32..262144).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        std::fs::write(&input, &bytes)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--ignored",
                "--exact",
                "lab::mirror::transport::tests::stream_child",
                "--nocapture",
            ])
            .env("ARA_LAB_STREAM_TEST_OUTPUT", &received);
        let output = run_input(&mut command, &root, &input, 10)?;
        assert!(output.contains("stream received"));
        assert_eq!(std::fs::read(received)?, bytes);
        let timing =
            super::super::super::session::read(&root.join("timing.json")).context("timing")?;
        assert_eq!(timing["input_bytes"], 262144);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    #[ignore = "subprocess helper for archive_stdin_preserves_binary_bytes"]
    fn stream_child() -> Result<()> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes)?;
        std::fs::write(
            std::env::var_os("ARA_LAB_STREAM_TEST_OUTPUT").context("test output")?,
            &bytes,
        )?;
        println!("stream received");
        Ok(())
    }
}
