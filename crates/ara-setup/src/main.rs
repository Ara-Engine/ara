//! `ara-setup`: the self-extracting setup of the Ara engine and Ara Hub. A double
//! click opens the setup window; `--silent`, `--list`, `--help` and `--version`
//! report to the parent console instead (see `ara_setup::args`).
#![windows_subsystem = "windows"]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    ara_setup::main()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("ara-setup runs on Windows");
    std::process::ExitCode::FAILURE
}
