//! Blender is optional at runtime and never part of an engine build or test.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub(super) fn render(
    root: &Path,
    base: &Path,
    args: &Value,
    activity: &super::track::Activity,
) -> Result<Value> {
    let _power = super::mac::Job::start()?;
    let _reference = activity.references.lock().expect("reference job");
    let scene = args["scene"].as_str().unwrap_or("showcase");
    ensure!(
        ["showcase", "outpost", "hangar"].contains(&scene),
        "unknown reference scene"
    );
    let bench = args["bench_root"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("D:/AraBench"));
    ensure!(
        bench
            .join("source")
            .join(scene)
            .join(format!("{scene}.blend"))
            .is_file(),
        "no Blender source for {scene} under {}; references cannot be rendered until a source exists",
        bench.display()
    );
    let blender = args["blender"]
        .as_str()
        .map(PathBuf::from)
        .or_else(|| {
            let candidate =
                PathBuf::from("C:/Program Files/Blender Foundation/Blender 5.1/blender.exe");
            candidate.is_file().then_some(candidate)
        })
        .unwrap_or_else(|| PathBuf::from("blender"));
    let samples = args["samples"].as_u64().unwrap_or(128);
    ensure!(
        (1..=8192).contains(&samples),
        "invalid reference sample count"
    );
    let output = base.join("references").join(scene).join("cycles");
    let job = base.join("references/jobs").join(super::session::id());
    fs::create_dir_all(&output)?;
    fs::create_dir_all(&job)?;
    let staging = job.join("cycles");
    fs::create_dir_all(&staging)?;
    let log = job.join("blender.log");
    let mut command = Command::new(blender);
    command
        .current_dir(root)
        .args(["-b", "--factory-startup", "--python"])
        .arg(root.join("benchmarks/cross-engine-showcase/scripts/render_reference.py"))
        .arg("--")
        .arg(scene)
        .arg("--bench-root")
        .arg(bench)
        .arg("--output-dir")
        .arg(&staging)
        .arg("--linear")
        .arg("--samples")
        .arg(samples.to_string())
        .env("BLENDER_USER_CONFIG", job.join("config"))
        .env("BLENDER_USER_SCRIPTS", job.join("scripts"))
        .env("BLENDER_USER_DATAFILES", job.join("data"))
        .stdin(std::process::Stdio::null())
        .stdout(fs::File::create(&log)?)
        .stderr(fs::File::options().append(true).open(&log)?);
    super::build::background(&mut command);
    let visual = crate::gpu::visual(3600)?;
    super::daemon::vram()?;
    let began = Instant::now();
    let mut child = crate::process::Tree::spawn(&mut command)
        .context("Blender 5.1 is an optional runtime tool; install it or supply --blender PATH")?;
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "Blender reference render failed; {}",
                log.display()
            );
            break;
        }
        ensure!(
            began.elapsed() < Duration::from_secs(3600),
            "Blender reference render timed out; {}",
            log.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(visual);
    let _publication = activity.gpu.write().expect("reference publication");
    super::variants::publish_tree(&staging, &output)?;
    Ok(
        json!({"scene":scene,"directory":output,"policy":super::session::read(&output.join("linear-reference.json")),"seconds":began.elapsed().as_secs_f64(),"log":log}),
    )
}
