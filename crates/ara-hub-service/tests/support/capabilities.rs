//! Windowless CLI shims exercise the real capability subprocess boundary.
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn engine_cli(directory: &Path, layouts: Option<&[u64]>) -> PathBuf {
    fs::create_dir_all(directory).expect("fixture CLI directory");
    let response = layouts.map(|layouts| {
        serde_json::json!({"engineVersion":"fixture","projectLayouts":layouts}).to_string()
    });
    let test_executable = std::env::current_exe().expect("test executable");
    #[cfg(windows)]
    let (name, script) = (
        "ara.cmd",
        format!(
            "@echo off\r\nif \"%~1\"==\"--json\" (\r\n  echo {}\r\n  exit /b {}\r\n)\r\nset \"ARA_HUB_FIXTURE_EDITOR=%~2\"\r\n\"{}\" --exact fixture_editor --nocapture\r\nexit /b %errorlevel%\r\n",
            response
                .as_deref()
                .unwrap_or("Unknown command: capabilities"),
            if response.is_some() { 0 } else { 1 },
            test_executable.display()
        ),
    );
    #[cfg(not(windows))]
    let (name, script) = (
        "ara",
        format!(
            "#!/bin/sh\nif [ \"$1\" = '--json' ]; then\n  printf '%s\\n' '{}'\n  exit {}\nfi\nexport ARA_HUB_FIXTURE_EDITOR=\"$2\"\nexec '{}' --exact fixture_editor --nocapture\n",
            response
                .as_deref()
                .unwrap_or("Unknown command: capabilities"),
            if response.is_some() { 0 } else { 1 },
            test_executable.to_string_lossy().replace('\'', "'\\''")
        ),
    );
    let path = directory.join(name);
    fs::write(&path, script).expect("fixture CLI script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("executable fixture CLI");
    }
    path
}
