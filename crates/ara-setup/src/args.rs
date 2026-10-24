//! The command line. A double click passes nothing and opens the window.
use std::{ffi::OsString, path::PathBuf};

/// What the process does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Install (the window, or the console with `--silent`).
    #[default]
    Install,
    /// Print the validated payload listing.
    List,
    /// Print usage.
    Help,
    /// Print the setup version.
    Version,
}

/// Parsed options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// What to do.
    pub mode: Mode,
    /// Installation directory (default: per payload kind, see `flow::Kind`).
    pub destination: Option<PathBuf>,
    /// No window: report to the console and exit with a status.
    pub silent: bool,
    /// Replace an existing Ara installation of the same kind without asking.
    pub replace: bool,
    /// No shortcuts, PATH, uninstall entry or shared Hub (automated checks).
    pub no_system_integration: bool,
}

impl Args {
    /// True when the process reports to a console instead of opening the window.
    pub fn console(&self) -> bool {
        self.silent || self.mode != Mode::Install
    }
}

/// `--help` text.
pub const USAGE: &str = "Install Ara for this Windows user.

Usage: ara-setup [--destination DIR] [--silent] [--replace] [--no-system-integration]
       ara-setup --list | --help | --version

  --destination DIR        Install into DIR (default: %LOCALAPPDATA%\\Programs\\Ara\\<version>,
                           or %LOCALAPPDATA%\\Programs\\Ara Hub for the Hub setup)
  --silent                 No window: print the result, exit 0 on success
  --replace                Replace an existing Ara installation of the same kind without
                           asking. Without it, --silent fails with ARA-EXP-INSTALL-004
  --no-system-integration  No shortcuts, PATH, uninstall entry or shared Hub
  --list                   Print the validated payload files
  --version                Print the setup version

Setup is a Windows program, so PowerShell does not wait for it. To wait and get the
exit code in $LASTEXITCODE, pipe it: .\\ara-setup.exe --silent | Out-Host";

/// Parses arguments (without the program name).
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if let Some(directory) = text.strip_prefix("--destination=") {
            out.destination = Some(PathBuf::from(directory));
            continue;
        }
        match text.as_ref() {
            "--destination" => {
                let directory = args.next().ok_or("--destination needs a directory")?;
                out.destination = Some(PathBuf::from(directory));
            }
            "--silent" => out.silent = true,
            "--replace" => out.replace = true,
            "--no-system-integration" => out.no_system_integration = true,
            "--list" => out.mode = Mode::List,
            "--help" | "-h" | "/?" => out.mode = Mode::Help,
            "--version" | "-V" => out.mode = Mode::Version,
            other => return Err(format!("unknown argument {other}; see --help")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn parse_all(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn double_click_opens_the_window_and_flags_select_the_console() {
        let none = parse_all(&[]).unwrap();
        assert_eq!(none, Args::default());
        assert!(!none.console());
        let smoke = parse_all(&[
            "--destination",
            "C:/x y",
            "--silent",
            "--no-system-integration",
        ])
        .unwrap();
        assert_eq!(smoke.destination, Some(PathBuf::from("C:/x y")));
        assert!(smoke.silent && smoke.no_system_integration && !smoke.replace);
        assert!(smoke.console());
        let inline = parse_all(&["--destination=D:/ara", "--replace"]).unwrap();
        assert_eq!(inline.destination, Some(PathBuf::from("D:/ara")));
        assert!(inline.replace && !inline.console());
        assert_eq!(parse_all(&["--list"]).unwrap().mode, Mode::List);
        assert_eq!(parse_all(&["/?"]).unwrap().mode, Mode::Help);
        assert!(parse_all(&["--destination"]).is_err());
        assert!(parse_all(&["--silent=1"]).is_err());
        assert!(parse_all(&["install"]).is_err());
    }
}
