//! DX replays workspace rustc directly, bypassing Cargo wrappers. Guard both routes.
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

pub(super) fn forward() -> Option<ExitCode> {
    let executable = std::env::current_exe().ok()?;
    if !matches!(
        executable.file_name()?.to_str(),
        Some("rustc" | "rustc.exe")
    ) {
        return None;
    }
    let compiler = std::env::var_os("ARA_HOTPATCH_RUSTC")?;
    Some(match compile(&compiler) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            // DX dependency replay keeps only rustc JSON diagnostics. Plain
            // stderr would discard the reason for a facade rejection.
            let message = format!("ARA7602: {error:#}");
            eprintln!(
                "{}",
                serde_json::json!({
                    "$message_type":"diagnostic", "message":message, "code":null,
                    "level":"error", "spans":[], "children":[],
                    "rendered":format!("{message}\n"),
                })
            );
            ExitCode::FAILURE
        }
    })
}

fn compile(compiler: &std::ffi::OsStr) -> Result<u8> {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    let crate_name = args
        .windows(2)
        .find(|a| a[0] == "--crate-name")
        .and_then(|a| a[1].to_str())
        .unwrap_or("probe")
        .to_owned();
    super::experiment::compiler(&crate_name, &mut args)?;
    if std::env::var("ARA_HOTPATCH_CRANELIFT_CRATE").is_ok_and(|name| name == crate_name) {
        args.push("-Zcodegen-backend=cranelift".into());
    }
    let began = std::time::Instant::now();
    ara_hotpatch::trace_event(
        "compiler_start",
        serde_json::json!({"crate":crate_name,"args":args.iter().filter_map(|a| a.to_str()).filter(|a| a.contains("incremental=") || a.contains("codegen-units=") || a.contains("debuginfo=") || a.contains("opt-level=") || a.contains("codegen-backend=") || a.contains("panic=")).collect::<Vec<_>>()}),
    );
    let source = args
        .iter()
        .find(|arg| Path::new(arg).extension().is_some_and(|e| e == "rs"));
    let contract = if source.is_some() {
        let path = std::env::var_os("ARA_HOTPATCH_CONTRACT")
            .ok_or_else(|| anyhow::anyhow!("source contract is missing"))?;
        Some(ara_hotpatch::Contract::read(&PathBuf::from(path))?)
    } else {
        None
    };
    if let (Some(contract), Some(source)) = (&contract, source) {
        contract.validate_source(Path::new(source))?;
    }
    ara_hotpatch::trace_event(
        "rustc_start",
        serde_json::json!({"crate":crate_name,"guard_us":began.elapsed().as_micros()}),
    );
    let compiling = std::time::Instant::now();
    let mut child = Command::new(compiler).args(&args).spawn()?;
    let status = child.wait()?;
    ara_hotpatch::trace_event(
        "rustc_end",
        serde_json::json!({"crate":crate_name,"compile_us":compiling.elapsed().as_micros(),"cpu_us":cpu_us(&child),"success":status.success()}),
    );
    if let (Some(contract), Some(source)) = (&contract, source) {
        contract.validate_source(Path::new(source))?;
    }
    ara_hotpatch::trace_event(
        "compiler_end",
        serde_json::json!({"crate":crate_name,"total_us":began.elapsed().as_micros()}),
    );
    Ok(status.code().unwrap_or(1) as u8)
}

#[cfg(windows)]
fn cpu_us(child: &std::process::Child) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetProcessTimes};
    let empty = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (empty, empty, empty, empty);
    // SAFETY: Child owns this process handle through the call; all output pointers
    // refer to separate initialized FILETIMEs that stay live until it returns.
    let ok = unsafe {
        GetProcessTimes(
            child.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    (ok != 0).then(|| (ticks(kernel) + ticks(user)) / 10)
}
#[cfg(not(windows))]
fn cpu_us(_: &std::process::Child) -> Option<u64> {
    None
}
