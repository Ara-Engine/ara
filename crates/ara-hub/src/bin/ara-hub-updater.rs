#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Runs from the config directory, never from the directory being replaced.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let request = std::path::PathBuf::from(args.next().ok_or("missing update plan")?);
    let pid: u32 = args
        .next()
        .ok_or("missing parent PID")?
        .to_str()
        .ok_or("invalid PID")?
        .parse()?;
    let plan: ara_update::updater::Plan = serde_json::from_slice(&std::fs::read(&request)?)?;
    ara_update::updater::wait_for_parent(pid)?;
    let result = ara_update::updater::apply(&plan);
    if result.is_ok() {
        let _ = std::fs::remove_file(&request);
        let _ = std::fs::remove_file(&plan.archive);
    }
    result.map_err(Into::into)
}
