use anyhow::{Context, Result, bail};
use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn run() -> Result<()> {
    if ara_crash::monitor_from_args()? {
        return Ok(());
    }
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("archive") => {
            let input = PathBuf::from(args.next().context("archive needs <pdb> <symbol-store>")?);
            let store = PathBuf::from(
                args.next()
                    .context("archive needs a symbol-store directory")?,
            );
            ara_crash::symbols::archive(&input, &store)?;
        }
        Some("symbolize") => {
            let dump = PathBuf::from(
                args.next()
                    .context("symbolize needs <dump> <symbols-directory>")?,
            );
            let symbols =
                PathBuf::from(args.next().context("symbolize needs <symbols-directory>")?);
            let report = ara_crash::symbols::symbolize(&dump, &symbols)?;
            println!(
                "{}",
                ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(
                    &report
                )?))
            );
        }
        Some("probe") => {
            let mode = args
                .next()
                .context("probe needs native|panic and a directory")?;
            let directory = PathBuf::from(args.next().context("probe directory missing")?);
            let _capture = ara_crash::Capture::start(&directory, "probe")?;
            match mode.as_str() {
                "native" => native_crash_probe(),
                "panic" => panic!("Ara panic capture probe"),
                "recovered" => {
                    let _ = std::panic::catch_unwind(|| panic!("Ara caught panic probe"));
                    native_crash_probe();
                }
                _ => bail!("unknown crash probe"),
            }
        }
        _ => bail!("Usage: ara-crash symbolize <dump> <symbols-directory>"),
    }
    Ok(())
}

#[inline(never)]
fn native_crash_probe() -> ! {
    ara_crash::native_crash_probe()
}
