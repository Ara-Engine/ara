//! Bin-only tip works around dx's lib+bin replay gap (#5778).
//! Normal path dependencies also keep the editor visible to dx's metadata watcher.
mod editor {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ara-editor/src/main.rs"
    ));

    pub(super) fn run() -> ara_ui::Result<()> {
        main()
    }
}

fn main() -> ara_ui::Result<()> {
    // Admit GPU work only after dx's initial build. This is the same exact-PID
    // descendant guard used by the render probe; the included editor stays intact.
    if cfg!(windows)
        && std::env::var_os("ARA_HOTPATCH_GPU_CHILD").is_none()
        && let Some(driver) = std::env::var_os("ARA_HOTPATCH_DRIVER")
    {
        // SAFETY: First operation at process entry, before any threads/runtimes.
        let arguments = unsafe { ara_hotpatch::args_os()? };
        let status = std::process::Command::new(driver)
            .args(["gpu", "--timeout", "1200", "--wait-for-quiet", "--"])
            .arg(std::env::current_exe()?)
            .args(arguments.iter().skip(1))
            .env("ARA_HOTPATCH_GPU_CHILD", "1")
            .status()?;
        if !status.success() {
            return Err(format!("GPU-queued editor failed: {status}").into());
        }
        return Ok(());
    }
    editor::run()
}
