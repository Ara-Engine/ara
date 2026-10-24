//! Ara setup: the program at the front of the engine and Hub self-extracting
//! installers (`ara-export-installer` appends the payload). A double click opens
//! the setup window ([`ui`]); command-line modes ([`args`]) report to the parent
//! console. Both run the same transaction ([`flow`]).
#![cfg(windows)]
pub mod args;
pub mod flow;
pub mod processes;
pub mod register;
pub mod ui;

use args::{Args, Mode, USAGE};
use std::process::ExitCode;
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    },
};

/// The `ara-setup` executable.
pub fn main() -> ExitCode {
    let exe = std::env::current_exe();
    let args = match args::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            if attach_console() {
                eprintln!("error: {message}\n\n{USAGE}");
            } else {
                let error = anyhow::anyhow!("{message}");
                ui::window::run(
                    Default::default(),
                    Args::default(),
                    Some(ui::View::failed(None, &error)),
                );
            }
            return ExitCode::from(2);
        }
    };
    let exe = match exe {
        Ok(exe) => exe,
        Err(error) => {
            attach_console();
            eprintln!("error: cannot locate the setup executable: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !args.console() {
        return if ui::window::run(exe, args, None) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    attach_console();
    match console(&exe, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Command-line modes: help, version, the payload listing and silent installs.
fn console(exe: &std::path::Path, args: &Args) -> anyhow::Result<()> {
    match args.mode {
        Mode::Help => println!("{USAGE}"),
        Mode::Version => println!("ara-setup {}", flow::VERSION),
        Mode::List => {
            let verified = ara_export_installer::Payload::open(exe)?.verify(|_| {})?;
            for name in verified.names() {
                println!("{name}");
            }
        }
        Mode::Install => match flow::run(exe, args, &mut flow::Quiet)? {
            flow::Outcome::Installed(installed) => {
                if !installed.output.is_empty() {
                    println!("{}", installed.output);
                }
                if let Some(leftover) = &installed.leftover {
                    eprintln!(
                        "warning: could not delete the replaced files at {}",
                        leftover.display()
                    );
                }
                println!(
                    "{} installed at {}",
                    installed.plan.kind.product(),
                    installed.plan.destination.display()
                );
            }
            flow::Outcome::Cancelled => anyhow::bail!("setup was cancelled"),
        },
    }
    Ok(())
}

/// Connects stdout/stderr to the parent's console, as a GUI-subsystem program
/// starts without one. Redirected handles (pipes, log files) are kept as they are.
/// Returns false when there is nowhere to write.
fn attach_console() -> bool {
    // SAFETY: handle queries and console attachment take no pointers.
    unsafe {
        let usable = |kind| {
            let handle = GetStdHandle(kind);
            !handle.is_null() && handle != INVALID_HANDLE_VALUE
        };
        usable(STD_OUTPUT_HANDLE)
            || usable(STD_ERROR_HANDLE)
            || AttachConsole(ATTACH_PARENT_PROCESS) != 0
    }
}
