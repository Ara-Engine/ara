//! Own dx baselines independently of GPU-admitted player launches.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Serialize, Deserialize)]
pub(super) struct Source {
    pub root: PathBuf,
    pub contract: PathBuf,
    pub app_directory: PathBuf,
    pub port: u16,
    pub directory: PathBuf,
}
struct Server {
    child: crate::process::Tree,
    directory: PathBuf,
}
#[derive(Default)]
pub(super) struct Sources(Mutex<Vec<Server>>);

impl Sources {
    pub(super) fn prepare(&self, root: &Path, dx: &Path, target: &Path) -> Result<PathBuf> {
        let mut servers = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("hotpatch source owner poisoned"))?;
        servers.retain_mut(|server| {
            let output =
                fs::read_to_string(server.directory.join("dx.out.log")).unwrap_or_default();
            server.child.try_wait().ok().flatten().is_none()
                && !server.directory.join("stopped").exists()
                && (output.contains("exited gracefully.") || !output.contains("Build failed"))
        });
        if let Some(server) = servers.last() {
            let manifest = server.directory.join("source.json");
            if !manifest.exists()
                || serde_json::from_slice::<Source>(&fs::read(&manifest)?).is_ok_and(|source| {
                    ara_hotpatch::Contract::read(&source.contract)
                        .is_ok_and(|c| c.validate().is_ok())
                })
            {
                return Ok(manifest);
            }
        }
        let generation = ara_hotpatch::timestamp_us().to_string();
        let directory = target.join("ara-hotpatch/lab").join(&generation);
        fs::create_dir_all(&directory)?;
        let port = std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port();
        let mut command = super::super::tool::command(std::env::current_exe()?);
        command
            .current_dir(root)
            .args(["hotpatch", "--dx"])
            .arg(dx)
            .args(["--port", &port.to_string(), "--", "player", "--help"])
            .env("CARGO_TARGET_DIR", target)
            .env("ARA_HOTPATCH_LAB_GENERATION", generation)
            .env("ARA_HOTPATCH_LAB_MANIFEST", directory.join("source.json"))
            .env("ARA_HOTPATCH_LAB_CONTROL", directory.join("bootstrap"))
            .env("ARA_NATIVE_PATCH_REQUEST", directory.join("request"))
            .stdout(fs::File::create(directory.join("dx.out.log"))?)
            .stderr(fs::File::create(directory.join("dx.err.log"))?);
        let child = crate::process::Tree::spawn(&mut command)?;
        let manifest = directory.join("source.json");
        servers.push(Server { child, directory });
        Ok(manifest)
    }
}

pub(super) fn wait(path: &Path) -> Result<Source> {
    let directory = path.parent().context("source directory")?;
    let began = std::time::Instant::now();
    loop {
        let output = fs::read_to_string(directory.join("dx.out.log")).unwrap_or_default();
        if output.contains("exited gracefully.") {
            let source: Source = serde_json::from_slice(&fs::read(path)?)?;
            ensure!(
                source.app_directory.is_dir(),
                "prepared dx app directory missing"
            );
            return Ok(source);
        }
        let error = fs::read_to_string(directory.join("dx.err.log")).unwrap_or_default();
        ensure!(
            !error.contains("error[E")
                && !error.contains("could not compile")
                && !error.contains("Error:")
                && !error.contains("ARA7600:")
                && !error.contains("ARA7602:")
                && !output.contains("Build failed"),
            "initial hotpatch build failed; see {}",
            directory.display()
        );
        ensure!(
            began.elapsed() < std::time::Duration::from_secs(2400),
            "initial hotpatch build timed out; see {}",
            directory.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
