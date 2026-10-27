//! Build-identified Preview-channel artifacts, with uploads explicitly opted in.
mod plan;
#[cfg(test)]
mod tests;
mod version;

use anyhow::{Context as _, Result, ensure};
use clap::Args;
use plan::Prepared;
use semver::Version;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

#[derive(Args)]
pub(crate) struct Options {
    /// Upload the signed Hub and engine prereleases; otherwise prepare locally only.
    #[arg(long)]
    publish: bool,
    /// Upload an already prepared preview record without another version bump or build.
    #[arg(long)]
    prepared: Option<PathBuf>,
    /// Shared warm Cargo outputs; e.g. D:\Coding\ara-after-freeze\target.
    #[arg(long)]
    target_dir: Option<PathBuf>,
    /// Explicit alpha.N version (otherwise choose the next workspace preview).
    #[arg(long)]
    version: Option<String>,
    /// GitHub owner/name; overrides ARA_RELEASE_REPO.
    #[arg(long)]
    repo: Option<String>,
    /// Signing-key file outside the checkout; defaults to ARA_RELEASE_SIGNING_KEY.
    #[arg(long)]
    key: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

trait Runner {
    fn execute(&mut self, label: &str, command: &mut Command, gpu: bool) -> Result<()>;
}

struct Native;
impl Runner for Native {
    fn execute(&mut self, label: &str, command: &mut Command, gpu: bool) -> Result<()> {
        let mut operation = || -> Result<()> {
            let outcome =
                crate::process::run(command, Some(Instant::now() + Duration::from_secs(7200)))?;
            ensure!(
                outcome.success(),
                "preview {label} failed or timed out; inspect its log"
            );
            Ok(())
        };
        if gpu {
            crate::gpu::locked(3600, operation)
        } else {
            operation()
        }
    }
}

struct Context<R> {
    run: PathBuf,
    source: PathBuf,
    target: PathBuf,
    key: PathBuf,
    runner: R,
}
impl<R: Runner> Context<R> {
    /// Log child output, not environments or command debug strings which may carry credentials.
    fn command(&mut self, label: &str, command: &mut Command, gpu: bool) -> Result<()> {
        let logs = self.run.join("logs");
        fs::create_dir_all(&logs)?;
        let log = logs.join(format!("{label}.log"));
        let output = File::create(&log)?;
        command
            .current_dir(&self.source)
            .env("CARGO_TARGET_DIR", &self.target)
            .env("CARGO_BUILD_BUILD_DIR", &self.target)
            .env(
                "CARGO_BUILD_JOBS",
                std::env::var_os("CARGO_BUILD_JOBS").unwrap_or_else(|| "2".into()),
            )
            .env("CARGO_PROFILE_RELEASE_DEBUG", "line-tables-only")
            .env("ARA_RELEASE_SIGNING_KEY", &self.key)
            .env("ARA_CONFIG_DIR", self.run.join("profile"))
            .env("ARA_CRASH_DIR", self.run.join("crashes"))
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .env_remove("ARA_SHARED_BUILD_DIR")
            .env_remove("GH_DEBUG")
            .env_remove("GIT_TRACE")
            .env_remove("GIT_TRACE_PACKET")
            .env_remove("GIT_TRACE_CURL")
            .env_remove("GIT_CURL_VERBOSE")
            .stdout(output.try_clone()?)
            .stderr(output);
        eprintln!("Preview {label}; {}", log.display());
        self.runner.execute(label, command, gpu)
    }
}

fn save(path: &Path, prepared: &Prepared) -> Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(prepared)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn key(root: &Path, options: &Options) -> Result<PathBuf> {
    let key = options
        .key
        .clone()
        .or_else(|| std::env::var_os("ARA_RELEASE_SIGNING_KEY").map(PathBuf::from))
        .context(
            "set ARA_RELEASE_SIGNING_KEY to the private signing-key file, or pass --key PATH",
        )?;
    ensure!(key.is_file(), "release signing-key file is unavailable");
    let key = key.canonicalize()?;
    ensure!(
        !key.starts_with(root.canonicalize()?),
        "release signing key must be outside the source checkout"
    );
    Ok(key)
}

fn prepare<R: Runner>(
    root: &Path,
    options: &Options,
    key: PathBuf,
    runner: R,
) -> Result<(Context<R>, Prepared, PathBuf)> {
    if options.target_dir.is_none() {
        crate::build_paths::Directories::from_env(root)?;
    }
    ensure!(
        crate::git::text(root, ["status", "--porcelain"])?.is_empty(),
        "commit source changes before preparing a preview"
    );
    let repo = options
        .repo
        .clone()
        .or_else(|| std::env::var("ARA_RELEASE_REPO").ok())
        .unwrap_or_else(|| "Ara-Engine/ara".into());
    plan::repository(&repo)?;
    let current = version::workspace_version(root)?;
    let runs = root.join("target/ara-dev/previews");
    fs::create_dir_all(&runs)?;
    let prepared = fs::read_dir(&runs)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<_>>>()?;
    let version = match &options.version {
        Some(value) => Version::parse(value)?,
        None => version::next(&current, prepared)?,
    };
    version::validate(&version, &current)?;
    let run = runs.join(version.to_string());
    fs::create_dir(&run).context("preview version already prepared or reserved; choose a new version or pass --prepared RECORD")?;
    fs::create_dir(run.join("profile"))?;
    let source = run.join("source");
    let head = crate::git::text(root, ["rev-parse", "HEAD"])?;
    let target = std::path::absolute(
        options
            .target_dir
            .clone()
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
            .unwrap_or_else(|| root.join("target")),
    )?;
    let mut context = Context {
        run,
        source,
        target,
        key,
        runner,
    };
    // A new independent clone keeps the caller's workspace and refs unchanged.
    let mut clone = Command::new("git");
    clone
        .args([
            "clone",
            "--no-local",
            "--no-checkout",
            "--single-branch",
            "--no-tags",
            "--",
        ])
        .arg(root)
        .arg(&context.source);
    let output = clone.output().context("create isolated preview checkout")?;
    ensure!(
        output.status.success(),
        "cannot create isolated preview checkout"
    );
    crate::git::bytes(&context.source, ["checkout", "--detach", &head])?;
    for field in ["user.name", "user.email"] {
        if let Ok(value) = crate::git::text(root, ["config", field]) {
            crate::git::bytes(&context.source, ["config", field, &value])?;
        }
    }
    let lock =
        version::external_resolutions(&fs::read_to_string(context.source.join("Cargo.lock"))?)?;
    version::bump(&context.source, &version, &head)?;
    // `cargo metadata --no-deps` never resolves, so it left Cargo.lock at the old
    // version and the release build rewrote it after the version commit.
    context.command(
        "lockfile",
        Command::new("cargo").args(["update", "--workspace", "--offline"]),
        false,
    )?;
    let bumped_lock = fs::read_to_string(context.source.join("Cargo.lock"))?;
    ensure!(
        version::external_resolutions(&bumped_lock)? == lock,
        "preview bump changed a third-party dependency resolution"
    );
    let stale = version::stale_local_entries(&bumped_lock, &current)?;
    ensure!(
        stale.is_empty(),
        "Cargo.lock still lists workspace packages at {current}: {}",
        stale.join(", ")
    );
    let changes = crate::git::text(&context.source, ["diff", "--name-only"])?;
    ensure!(
        changes
            .lines()
            .all(|name| matches!(name, "Cargo.toml" | "Cargo.lock" | "RELEASE_NOTES.md")),
        "version preparation changed files outside its three-file scope"
    );
    crate::git::bytes(
        &context.source,
        ["add", "--", "Cargo.toml", "Cargo.lock", "RELEASE_NOTES.md"],
    )?;
    crate::git::bytes(
        &context.source,
        ["commit", "-m", &format!("Prepare preview {version}")],
    )?;
    let commit = crate::git::text(&context.source, ["rev-parse", "HEAD"])?;
    context.command(
        "release-tool",
        Command::new("cargo").args(["build", "--release", "-p", "ara-release"]),
        false,
    )?;
    let tools = context.run.join("tools");
    fs::create_dir(&tools)?;
    let tool = tools.join(format!("ara-release{}", std::env::consts::EXE_SUFFIX));
    fs::copy(
        context
            .target
            .join(std::env::var_os("CARGO_BUILD_TARGET").unwrap_or_default())
            .join("release")
            .join(tool.file_name().context("tool filename")?),
        &tool,
    )?;
    // Sample content is fetched, not tracked (samples/.gitignore), and packaging
    // builds the sample kits from it. Seed the clone from the caller's blob cache so
    // fetch stays offline when it can, then fetch and verify against assets.lock.json.
    let blobs = root.join("samples/.cache/blobs");
    if blobs.is_dir() {
        crate::d26::copy_tree(&blobs, &context.source.join("samples/.cache/blobs"))?;
    }
    for step in ["fetch", "verify"] {
        context.command(
            &format!("samples-{step}"),
            Command::new("cargo").args(["run", "--release", "-p", "ara-samples", "--", step]),
            false,
        )?;
    }
    let artifacts = context.source.join("target/release-dist");
    let mut prepared = Prepared {
        format: "ara.preview/1".into(),
        version: version.to_string(),
        repo,
        source_head: head,
        version_commit: commit,
        source: context.source.clone(),
        target_dir: context.target.clone(),
        tool: tool.clone(),
        tool_hash: crate::git::hash_file(root, &tool)?,
        artifacts,
        smoke_passed: false,
        hub_published: false,
        engine_published: false,
    };
    context.command(
        "package",
        Command::new(&tool).args(["package", "--kind", "all", "--version", &prepared.version]),
        false,
    )?;
    context.command(
        "smoke",
        Command::new(&tool)
            .args(["smoke", "--version", &prepared.version, "--artifacts"])
            .arg(&prepared.artifacts),
        true,
    )?;
    prepared.smoke_passed = true;
    for kind in ["hub", "engine"] {
        context.command(
            &format!("sign-{kind}"),
            Command::new(&tool).args(plan::publish_args(&prepared, kind, false)),
            false,
        )?;
    }
    let record = context.run.join("preview.json");
    save(&record, &prepared)?;
    Ok((context, prepared, record))
}

pub(crate) fn run(root: &Path, options: Options) -> Result<bool> {
    workflow(root, options, Native)
}

fn workflow<R: Runner>(root: &Path, options: Options, runner: R) -> Result<bool> {
    let key = key(root, &options)?;
    let (mut context, mut prepared, record) = if let Some(record) = &options.prepared {
        let record = std::path::absolute(record)?;
        ensure!(
            options.version.is_none() && options.target_dir.is_none() && options.repo.is_none(),
            "--prepared uses the recorded version, target and repository"
        );
        let prepared: Prepared = serde_json::from_slice(&fs::read(&record)?)?;
        ensure!(
            prepared.format == "ara.preview/1" && prepared.smoke_passed,
            "preview record has no successful smoke test"
        );
        plan::repository(&prepared.repo)?;
        ensure!(
            version::workspace_version(&prepared.source)?.to_string() == prepared.version,
            "prepared workspace version changed"
        );
        ensure!(
            crate::git::text(&prepared.source, ["rev-parse", "HEAD"])? == prepared.version_commit,
            "prepared source changed"
        );
        ensure!(
            crate::git::text(
                &prepared.source,
                ["status", "--porcelain", "--untracked-files=no"]
            )?
            .is_empty(),
            "prepared source has uncommitted changes"
        );
        ensure!(
            crate::git::hash_file(root, &prepared.tool)? == prepared.tool_hash,
            "prepared release tool changed"
        );
        let context = Context {
            run: record.parent().context("preview directory")?.to_owned(),
            source: prepared.source.clone(),
            target: prepared.target_dir.clone(),
            key,
            runner,
        };
        (context, prepared, record)
    } else {
        prepare(root, &options, key, runner)?
    };
    // Recheck every signed folder before any remote write, including resumed publication.
    if options.prepared.is_some() {
        for kind in ["hub", "engine"] {
            context.command(
                &format!("verify-{kind}"),
                Command::new(&prepared.tool).args(plan::publish_args(&prepared, kind, false)),
                false,
            )?;
        }
    }
    if options.publish {
        context.command(
            "source-push",
            Command::new("git")
                .args(["push", "--porcelain"])
                .arg(format!("https://github.com/{}.git", prepared.repo))
                .arg(format!("HEAD:refs/heads/previews/{}", prepared.version)),
            false,
        )?;
        for kind in ["hub", "engine"] {
            if (kind == "hub" && prepared.hub_published)
                || (kind == "engine" && prepared.engine_published)
            {
                continue;
            }
            context.command(
                &format!("publish-{kind}"),
                Command::new(&prepared.tool).args(plan::publish_args(&prepared, kind, true)),
                false,
            )?;
            if kind == "hub" {
                prepared.hub_published = true;
            } else {
                prepared.engine_published = true;
            }
            save(&record, &prepared)?;
        }
    }
    if options.json {
        println!("{}", serde_json::to_string_pretty(&prepared)?);
    } else {
        println!(
            "Preview {}: {}\nRecord: {}",
            prepared.version,
            if options.publish {
                "published"
            } else {
                "signed locally; nothing uploaded"
            },
            record.display()
        );
        if !options.publish {
            println!(
                "Publish: ara-dev preview --prepared \"{}\" --publish",
                record.display()
            );
        }
    }
    Ok(true)
}
