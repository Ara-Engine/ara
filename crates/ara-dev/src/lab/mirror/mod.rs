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
    #[serde(default)]
    pub slot_granted: bool,
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
            &std::fs::read(if base.join("mirror/config.json").is_file() {
                base.join("mirror/config.json")
            } else {
                base.join("mirror/queued-config.json")
            })
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
        slot_granted: args["grant_slot"].as_bool().unwrap_or(false),
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
    sources: Arc<AtomicBool>,
}
impl Worker {
    pub fn new(root: &Path, base: &Path, activity: Arc<super::track::Activity>) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let sources = Arc::new(AtomicBool::new(true));
        let source_pending = Arc::clone(&sources);
        let (root, base, cancel) = (root.to_owned(), base.to_owned(), Arc::clone(&stop));
        let run = move || {
            let mut pending = base.join("mirror/config.json").is_file()
                || base.join("mirror/queued-config.json").is_file();
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
                let source_update = source_pending.swap(false, Ordering::AcqRel);
                let outcome = sync(&root, &base, &activity, source_update);
                pending = outcome.is_err();
                if let Err(error) = outcome {
                    source_pending.store(true, Ordering::Release);
                    let value = json!({"phase":"pending","error":format!("{error:#}"),"at":crate::d26::validity::unix_ms().unwrap_or_default()});
                    let _ = super::session::write(&base.join("mirror/status.json"), &value);
                }
            }
        };
        std::thread::Builder::new()
            .name("ara-lab-mac-mirror".into())
            .spawn(run)?;
        Ok(Self {
            send,
            stop,
            sources,
        })
    }
    pub fn notify(&self) {
        let _ = self.send.try_send(());
    }
    pub fn notify_sources(&self) {
        self.sources.store(true, Ordering::Release);
        self.notify();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn sync(
    root: &Path,
    base: &Path,
    activity: &super::track::Activity,
    source_update: bool,
) -> Result<()> {
    let config = Config::load(base)?;
    config.validate()?;
    if !config.slot_granted {
        super::session::write(
            &base.join("mirror/status.json"),
            &json!({"phase":"queued_for_mac_slot","build_started":false,"detail":"Mac preview verification has priority; wait for the user's slot grant"}),
        )?;
        return Ok(());
    }
    let power = transport::ssh(&config, base, "/usr/bin/pmset -g batt\n", 20)?;
    super::mac::require_ac(&power)?;
    let began = std::time::Instant::now();
    let previous = super::session::read(&base.join("mirror/status.json")).unwrap_or(json!({}));
    let source_update = source_update || !base.join("mirror/peer.json").is_file();
    let mut commit = previous["snapshot"].clone();
    let mut status = previous["peer"].clone();
    let progress = |phase: &str| -> Result<()> {
        super::session::write(
            &base.join("mirror/status.json"),
            &json!({"phase":phase,"source_sync":source_update,"elapsed_seconds":began.elapsed().as_secs_f64(),"at":crate::d26::validity::unix_ms().unwrap_or_default()}),
        )
    };
    if source_update {
        progress("source_snapshot")?;
        let revision = snapshot::create(root, base, &config.source_ref)?;
        let fetch = format!(
            "{}if /usr/bin/pgrep -x cargo >/dev/null; then echo 'Mac build lane is occupied; checkout retained' >&2; exit 1; fi\ntest -z \"$(git status --porcelain --untracked-files=no)\" || {{ echo 'Mac checkout is dirty; keep its edits and resolve them before mirroring' >&2; exit 1; }}\ngit log -1 --format='Mac checkout before lab sync: %H %D' >&2\ngit fetch --no-tags {} {} {}\nif test \"$(git rev-parse HEAD)\" != {}; then git checkout --detach {}; fi\n",
            transport::prelude(&config),
            transport::quote(&config.pc_repository),
            transport::quote(&format!(
                "+refs/heads/{}:refs/remotes/ara-lab/fidelity-lab",
                config.branch
            )),
            transport::quote(&format!("+{}:{}", config.source_ref, config.source_ref)),
            transport::quote(&revision),
            transport::quote(&config.source_ref)
        );
        progress("source_fetch")?;
        transport::ssh(&config, base, &fetch, 90)?;
        commit = json!(revision);
    }
    progress("cooked_transfer")?;
    bundle::push(root, base, &config, activity)?;
    if source_update {
        progress("peer_bootstrap")?;
        let output = transport::ssh(&config, base, &peer::bootstrap(&config), 1200)?;
        status = serde_json::from_str(output.trim())?;
    }
    if let Some(import) = super::session::read(&base.join("mirror/incoming-local.json")) {
        transport::rpc(&config, base, "_lab_import", import)?;
    }
    if let Some(pending) = super::session::read(&base.join("mirror/selections.json"))
        && let Some(pending) = pending.as_object()
    {
        let mut remaining = pending.clone();
        let variants = super::variants::Variants::load(base);
        for (track, name) in pending {
            let name = name.as_str().context("pending variant name")?;
            // Registration can precede its cook. Publication notifies us again;
            // pending or failed experiments must not force a Rust source sync.
            if variants.ready(name).is_err() {
                continue;
            }
            transport::rpc(
                &config,
                base,
                "lab_variant",
                json!({"session":track,"name":name}),
            )?;
            remaining.remove(track);
        }
        super::session::write(&base.join("mirror/selections.json"), &remaining)?;
    }
    super::session::write(
        &base.join("mirror/peer.json"),
        &json!({"pid":status["pid"],"host":config.host,"repo":config.repo}),
    )?;
    super::session::write(
        &base.join("mirror/status.json"),
        &json!({"phase":"ready","snapshot":commit,"peer":status,"source_sync":source_update,"transport_seconds":began.elapsed().as_secs_f64(),"cook_host":"pc","at":crate::d26::validity::unix_ms().unwrap_or_default()}),
    )
}

pub(super) fn proxy(root: &Path, tool: &str, mut args: Value) -> Result<Value> {
    let base = super::directory(root);
    let config = Config::load(&base)?;
    config.validate()?;
    ensure!(
        config.slot_granted || ["lab_status", "lab_logs"].contains(&tool),
        "Mac mirror queued for a machine slot; wait for the user's slot grant before enabling lab mirror --grant-slot"
    );
    args.as_object_mut()
        .context("tool arguments")?
        .remove("platform");
    if !config.slot_granted {
        return Ok(
            json!({"platform":"mac","cached_only":true,"mirror":super::session::read(&base.join("mirror/status.json"))}),
        );
    }
    let mut value = transport::rpc(&config, &base, tool, bundle::portable(args))?;
    if ["lab_capture", "lab_measure", "lab_ab"].contains(&tool) {
        bundle::pull(&config, &base, &mut value)?;
    }
    value["platform"] = json!("mac");
    value["mirror"] = super::session::read(&base.join("mirror/status.json")).unwrap_or(Value::Null);
    Ok(value)
}

pub(super) fn require_slot(base: &Path) -> Result<()> {
    ensure!(
        Config::load(base)?.slot_granted,
        "Mac mirror is queued for the user's machine slot; no remote job started"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coalescing_artifact_notices_cannot_lose_a_rust_update() {
        let (send, receive) = mpsc::sync_channel(1);
        let worker = Worker {
            send,
            stop: Arc::new(AtomicBool::new(false)),
            sources: Arc::new(AtomicBool::new(false)),
        };
        worker.notify();
        worker.notify_sources();
        assert!(receive.try_recv().is_ok());
        assert!(worker.sources.load(Ordering::Acquire));
    }
    #[test]
    fn an_ungranted_slot_never_starts_transport_or_source_snapshotting() -> Result<()> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(super::super::session::id());
        std::fs::create_dir_all(base.join("mirror"))?;
        let config = json!({"slot_granted":false,"host":"unused.invalid","repo":"/repo","build_dir":"/repo/target","pc_repository":"pc:D:/repo","branch":"lab","source_ref":"refs/ara-lab/test"});
        super::super::session::write(&base.join("mirror/config.json"), &config)?;
        sync(
            &base,
            &base,
            &super::super::track::Activity::default(),
            true,
        )?;
        assert!(!base.join("mirror/jobs").exists());
        assert!(!base.join("mirror/git").exists());
        assert_eq!(
            super::super::session::read(&base.join("mirror/status.json")).context("status")?["phase"],
            "queued_for_mac_slot"
        );
        std::fs::remove_dir_all(base)?;
        Ok(())
    }
    #[test]
    fn old_configs_do_not_implicitly_claim_a_mac_slot() -> Result<()> {
        let config: Config = serde_json::from_value(
            json!({"host":"mac","repo":"/repo","build_dir":"/repo/target","pc_repository":"pc:D:/repo","branch":"lab","source_ref":"refs/ara-lab/test"}),
        )?;
        assert!(!config.slot_granted);
        config.validate()?;
        Ok(())
    }
}
