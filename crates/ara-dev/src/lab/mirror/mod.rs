//! PC-owned source/cooked-artifact transport; the peer runs the same lab protocol.
pub(super) mod bundle;
mod peer;
mod snapshot;
mod transport;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Config {
    pub host: String,
    pub repo: String,
    pub build_dir: String,
    pub pc_repository: String,
    pub branch: String,
    pub source_ref: String,
}
impl Config {
    fn load(base: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(
            &std::fs::read(base.join("mirror/config.json"))
                .context("configure the Mac with lab mirror first")?,
        )?)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.host.is_empty()
                && self
                    .host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "@.-_".contains(c))
                && !self.host.starts_with('-'),
            "invalid SSH host"
        );
        for path in [&self.repo, &self.build_dir] {
            ensure!(
                path.starts_with('/')
                    && !path.contains(['\n', '\r', '\0'])
                    && !Path::new(path)
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir)),
                "remote paths must be absolute without parent traversal"
            );
        }
        Ok(())
    }
}
pub(super) fn configure(root: &Path, base: &Path, args: &Value) -> Result<Value> {
    let repo = args["repo"]
        .as_str()
        .unwrap_or("/Users/islam/projects/ara")
        .to_owned();
    let common = crate::git::text(root, ["rev-parse", "--git-common-dir"])?;
    let common = root.join(common.trim()).canonicalize()?;
    let repository = common
        .parent()
        .context("PC Git repository")?
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    let value = Config {
        host: args["host"].as_str().unwrap_or("mac").into(),
        build_dir: args["build_dir"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{repo}/target")),
        repo,
        pc_repository: args["pc_repository"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("pc:{repository}")),
        branch: crate::git::text(root, ["branch", "--show-current"])?
            .trim()
            .into(),
        source_ref: format!("refs/ara-lab/{}", super::session::id()),
    };
    value.validate()?;
    ensure!(
        !value.branch.is_empty(),
        "configure the lab on its named branch"
    );
    std::fs::create_dir_all(base.join("mirror"))?;
    super::session::write(&base.join("mirror/config.json"), &value)?;
    Ok(json!({"configured":value,"queued":true}))
}

pub(super) fn select_after_cook(base: &Path, args: &Value) -> Result<()> {
    std::fs::create_dir_all(base.join("mirror"))?;
    let mut pending =
        super::session::read(&base.join("mirror/selections.json")).unwrap_or(json!({}));
    pending[args["session"].as_str().unwrap_or("default")] = args["name"].clone();
    super::session::write(&base.join("mirror/selections.json"), &pending)
}

pub(super) struct Worker {
    send: mpsc::SyncSender<()>,
    stop: Arc<AtomicBool>,
}
impl Worker {
    pub fn new(root: &Path, base: &Path, activity: Arc<super::track::Activity>) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let (root, base, cancel) = (root.to_owned(), base.to_owned(), Arc::clone(&stop));
        let run = move || {
            let mut pending = base.join("mirror/config.json").is_file();
            while !cancel.load(Ordering::Acquire) {
                let delay = if pending {
                    Duration::from_secs(15)
                } else {
                    Duration::from_secs(1)
                };
                match receive.recv_timeout(delay) {
                    Ok(()) => pending = base.join("mirror/config.json").is_file(),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if !pending {
                    continue;
                }
                while receive.try_recv().is_ok() {}
                let outcome = sync(&root, &base, &activity);
                pending = outcome.is_err();
                if let Err(error) = outcome {
                    let value = json!({"phase":"pending","error":format!("{error:#}"),"at":crate::d26::validity::unix_ms().unwrap_or_default()});
                    let _ = super::session::write(&base.join("mirror/status.json"), &value);
                }
            }
        };
        std::thread::Builder::new()
            .name("ara-lab-mac-mirror".into())
            .spawn(run)?;
        Ok(Self { send, stop })
    }
    pub fn notify(&self) {
        let _ = self.send.try_send(());
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn sync(root: &Path, base: &Path, activity: &super::track::Activity) -> Result<()> {
    let config = Config::load(base)?;
    config.validate()?;
    let power = transport::ssh(&config, base, "/usr/bin/pmset -g batt\n", 20)?;
    super::mac::require_ac(&power)?;
    let commit = snapshot::create(root, base, &config.source_ref)?;
    let fetch = format!(
        "{}test -z \"$(git status --porcelain --untracked-files=no)\" || {{ echo 'Mac checkout is dirty; keep its edits and resolve them before mirroring' >&2; exit 1; }}\ngit fetch --no-tags {} {} {}\nif test \"$(git rev-parse HEAD)\" != {}; then git checkout --detach {}; fi\n",
        transport::prelude(&config),
        transport::quote(&config.pc_repository),
        transport::quote(&format!(
            "+refs/heads/{}:refs/remotes/ara-lab/fidelity-lab",
            config.branch
        )),
        transport::quote(&format!("+{}:{}", config.source_ref, config.source_ref)),
        transport::quote(&commit),
        transport::quote(&config.source_ref)
    );
    transport::ssh(&config, base, &fetch, 90)?;
    bundle::push(root, base, &config, activity)?;
    let output = transport::ssh(&config, base, &peer::bootstrap(&config), 1200)?;
    let status: Value = serde_json::from_str(output.trim())?;
    if let Some(import) = super::session::read(&base.join("mirror/incoming-local.json")) {
        transport::rpc(&config, base, "_lab_import", import)?;
    }
    if let Some(pending) = super::session::read(&base.join("mirror/selections.json"))
        && let Some(pending) = pending.as_object()
    {
        for (track, name) in pending {
            let name = name.as_str().context("pending variant name")?;
            super::variants::Variants::load(base).ready(name)?;
            transport::rpc(
                &config,
                base,
                "lab_variant",
                json!({"session":track,"name":name}),
            )?;
        }
        super::session::write(&base.join("mirror/selections.json"), &json!({}))?;
    }
    super::session::write(
        &base.join("mirror/peer.json"),
        &json!({"pid":status["pid"],"host":config.host,"repo":config.repo}),
    )?;
    super::session::write(
        &base.join("mirror/status.json"),
        &json!({"phase":"ready","snapshot":commit,"peer":status,"cook_host":"pc","at":crate::d26::validity::unix_ms().unwrap_or_default()}),
    )
}

pub(super) fn proxy(root: &Path, tool: &str, mut args: Value) -> Result<Value> {
    let base = super::directory(root);
    let config = Config::load(&base)?;
    config.validate()?;
    args.as_object_mut()
        .context("tool arguments")?
        .remove("platform");
    let mut value = transport::rpc(&config, &base, tool, bundle::portable(args))?;
    if ["lab_capture", "lab_measure", "lab_ab"].contains(&tool) {
        bundle::pull(&config, &base, &mut value)?;
    }
    value["platform"] = json!("mac");
    value["mirror"] = super::session::read(&base.join("mirror/status.json")).unwrap_or(Value::Null);
    Ok(value)
}
