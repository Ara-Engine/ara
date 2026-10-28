//! Atomic capture evidence: an interrupted or failed run cannot leave valid old numbers.
use anyhow::Result;
use ara_bench::passes::Frame;
use ara_runtime_render::Presenter;
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn frame(presenter: &Presenter) -> Option<Frame> {
    if presenter.stats().skipped || presenter.stats().gpu.is_none() {
        return None;
    }
    presenter
        .renderer()
        .map(|renderer| Frame::from_names(renderer.timings().iter().map(|pass| pass.pass.as_str())))
}
pub(super) fn record(path: &Path, render: impl FnOnce() -> Result<Value>) -> Result<Value> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write(path, &json!({"completed":false,"status":"incomplete"}))?;
    let result = render();
    match &result {
        Ok(value) => write(path, value)?,
        Err(error) => {
            let presence = error
                .downcast_ref::<ara_bench::passes::PassError>()
                .map(|error| error.evidence());
            write(
                path,
                &json!({"completed":false,"status":"invalid","diagnostic":error.to_string(),"pass_presence":presence}),
            )?;
        }
    }
    result
}
fn write(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("timing.tmp");
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ara_bench::passes::{Expected, Guard};
    #[test]
    fn an_ssr_fallback_replaces_old_valid_capture_timings_with_a_coded_failure() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-samples/tests/pass-evidence")
            .join(std::process::id().to_string())
            .join("bench-timings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write(&path, &json!({"completed":true,"p50":0.43})).unwrap();
        let rt = [
            "RT.Reflect",
            "RT.Filter.X",
            "RT.Filter.Y",
            "RT.Rough",
            "RT.Rough.X",
            "RT.Rough.Y",
        ];
        let mut guard = Guard::new(Expected::Hybrid, 1, std::time::Duration::from_secs(1));
        guard
            .wait(std::time::Duration::ZERO, Some(Frame::from_names(rt)))
            .unwrap();
        guard.record(Frame::from_names(rt)).unwrap();
        let result = record(&path, || {
            let pending: Value = serde_json::from_slice(&fs::read(&path)?).unwrap();
            assert_eq!(pending["status"], "incomplete");
            assert!(pending["p50"].is_null());
            guard.record(Frame::from_names(["SSR"]))?;
            Ok(json!({"p50":0.43}))
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .starts_with(&format!("{}:", ara_bench::passes::INVALID_PASS_SET))
        );
        let failed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(failed["status"], "invalid");
        assert_eq!(failed["completed"], false);
        assert_eq!(failed["pass_presence"]["rt_present"], "1/2 frames");
        assert!(failed["p50"].is_null());
    }
}
