//! Offline lab gate client; the GPU worker is an explicitly selected executable.
mod attribution;
mod catalog;
mod contract;
mod gates;
mod report;
mod window;

use anyhow::{Context, Result, ensure};
use clap::Args;
use contract::{Case, RunResult};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Args)]
pub(crate) struct Options {
    /// G1 through G6, or all. Reports are written in separate G<n> directories.
    #[arg(long, default_value = "all")]
    gate: String,
    /// Print the versioned case catalogue without starting a GPU process.
    #[arg(long)]
    list: bool,
    /// Prebuilt render_v3 worker (or a wave-2 worker implementing the same contract).
    #[arg(long)]
    runner: Option<PathBuf>,
    #[arg(long, default_value = "D:/AraBuild/render-v3/gates")]
    output: PathBuf,
    /// Comma-separated case ids. A subset can never pass an entire gate.
    #[arg(long, value_delimiter = ',')]
    cases: Vec<String>,
    /// Read existing run.json files; no GPU work.
    #[arg(long)]
    report_only: bool,
    /// Capture/reference work only; skip timing. Missing cost remains n/a.
    #[arg(long)]
    quality_only: bool,
    /// Timing replay only; references can be produced later without a quiet lease.
    #[arg(long, conflicts_with = "quality_only")]
    timing_only: bool,
    #[arg(long, default_value_t = 1920)]
    width: u32,
    #[arg(long, default_value_t = 1080)]
    height: u32,
    #[arg(long, default_value_t = 256)]
    reference_samples: u32,
    /// Render independent references for every frame instead of sampled evidence.
    #[arg(long)]
    all_references: bool,
    /// Per-worker timeout, including cold creation. A timed out case is retained.
    #[arg(long, default_value_t = 600)]
    timeout: u64,
}

pub(crate) fn run(root: &Path, options: Options) -> Result<bool> {
    let suite = catalog::suite()?;
    if options.list {
        println!("{}", serde_json::to_string_pretty(&suite)?);
        return Ok(true);
    }
    let gates: Vec<u8> = if options.gate == "all" {
        (1..=6).collect()
    } else {
        vec![
            options
                .gate
                .trim_start_matches(['G', 'g'])
                .parse()
                .context("gate must be G1..G6 or all")?,
        ]
    };
    ensure!(
        gates.iter().all(|g| (1..=6).contains(g)),
        "gate must be G1..G6"
    );
    for id in &options.cases {
        ensure!(suite.cases.iter().any(|c| &c.id == id), "unknown case {id}");
    }
    let selected: Vec<_> = suite
        .cases
        .iter()
        .filter(|c| {
            c.gates.iter().any(|g| gates.contains(g))
                && (options.cases.is_empty() || options.cases.contains(&c.id))
        })
        .collect();
    ensure!(!selected.is_empty(), "no selected cases");
    fs::create_dir_all(&options.output)?;
    let cache = options.output.join("cases");
    fs::create_dir_all(&cache)?;
    if !options.report_only {
        let runner=options.runner.as_ref().context("build render_v3 once, then supply --runner PATH; lab gate never silently builds renderer code")?;
        ensure!(
            runner.is_file(),
            "worker does not exist: {}",
            runner.display()
        );
        for case in &selected {
            write_json(&cache.join(format!("{}.json", case.id)), case)?;
            write_json(
                &cache.join(format!("{}.script.json", case.id)),
                &case.script(suite.fps),
            )?;
        }
        if !options.quality_only {
            let _window = window::QuietWindow::acquire()?;
            let start = Instant::now();
            let mut attempted = Vec::new();
            window::telemetry(&options.output.join("nvidia-before.log"))?;
            let operation =
                crate::gpu::locked_with(30, false, crate::gpu_guard::Policy::default(), || {
                    for case in &selected {
                        if start.elapsed() > Duration::from_secs(2400) {
                            break;
                        }
                        attempted.push(case.id.as_str());
                        execute(
                            root,
                            &options,
                            runner,
                            &cache,
                            case,
                            "uncapped",
                            Some(Duration::from_secs(2500).saturating_sub(start.elapsed())),
                        )?;
                    }
                    Ok(())
                });
            window::telemetry(&options.output.join("nvidia-after.log"))?;
            let reading = crate::gpu::last_reading();
            write_json(&options.output.join("timing-guard.json"), &reading)?;
            // Publication happens after the foreign-load guard closes; child output
            // cannot self-certify a timing window as uncontaminated.
            for id in attempted {
                let path = cache.join(id).join("uncapped/run.json");
                if path.is_file() {
                    let mut run: RunResult = serde_json::from_slice(&fs::read(&path)?)?;
                    run.timing_valid = operation.is_ok();
                    write_json(&path, &run)?;
                }
            }
            if let Err(error) = operation {
                write_json(
                    &options.output.join("timing-error.json"),
                    &format!("{error:#}"),
                )?;
                eprintln!("timing unavailable: {error:#}");
            }
        }
        if !options.timing_only {
            for case in &selected {
                window::check_foreign()?;
                let _visual = crate::gpu::visual(30)?;
                execute(root, &options, runner, &cache, case, "capped", None)?;
            }
        }
    }
    for gate in gates {
        report::write(gate, &suite, &options.output)?;
    }
    println!(
        "{}",
        serde_json::json!({"output":options.output,"promotion":false,"note":"Per-device evidence only; all gate cases on both target GPUs are required."})
    );
    Ok(true)
}

fn execute(
    root: &Path,
    options: &Options,
    runner: &Path,
    cache: &Path,
    case: &Case,
    kind: &str,
    remaining: Option<Duration>,
) -> Result<()> {
    let out = cache.join(&case.id).join(kind);
    fs::create_dir_all(&out)?;
    // Preserve old receipts but never score one as this attempt after a failure.
    for name in ["run.json", "error.json"] {
        let path = out.join(name);
        if path.exists() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            fs::rename(&path, out.join(format!("previous-{stamp}-{name}")))?;
        }
    }
    let mut command = Command::new(runner);
    command
        .current_dir(root)
        .arg(cache.join(format!("{}.json", case.id)))
        .arg(kind)
        .arg(&out)
        .arg(options.width.to_string())
        .arg(options.height.to_string())
        .arg(options.reference_samples.to_string())
        .env("ARA_CONFIG_DIR", out.join("profile/config"))
        .env("ARA_CRASH_DIR", out.join("profile/crashes"))
        .env("ARA_RENDER_V3_TIMING_VALID", "0")
        .stdin(Stdio::null())
        .stdout(fs::File::create(out.join("stdout.log"))?)
        .stderr(fs::File::create(out.join("stderr.log"))?);
    if options.all_references {
        command.arg("all-references");
    }
    super::build::background(&mut command);
    let began = Instant::now();
    let mut child = crate::process::Tree::spawn(&mut command)?;
    let error = loop {
        if let Some(status) = child.try_wait()? {
            break (!status.success()).then(|| format!("worker exited {status}"));
        }
        if began.elapsed()
            > remaining
                .unwrap_or(Duration::from_secs(3600))
                .min(Duration::from_secs(options.timeout))
        {
            break Some("worker timeout; owned process tree terminated".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    drop(child);
    if let Some(error) = error {
        write_json(&out.join("error.json"), &error)?;
        eprintln!("{} {kind}: {error}", case.id);
    } else {
        eprintln!("{} {kind}: complete", case.id);
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
