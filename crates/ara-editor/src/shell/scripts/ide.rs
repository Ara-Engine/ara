//! Literal IDE argv, retained before policy decides whether a process may launch.
use super::*;

/// The last requested invocation, including suppression and any attempted fallback.
/// `launched` means process creation succeeded, not that the editor delivered a location.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdeRequest {
    target_file: String,
    line: Option<u32>,
    column: Option<u32>,
    program: String,
    args: Vec<String>,
    cwd: String,
    line_handling: &'static str,
    launch_suppressed: bool,
    attempted: bool,
    outcome: &'static str,
    suppression_reason: Option<&'static str>,
    error: Option<String>,
    fallback: Option<Box<IdeRequest>>,
}

#[derive(Clone, Copy)]
struct LaunchPolicy {
    suppressed: Option<&'static str>,
}

impl LaunchPolicy {
    fn new(offscreen: bool, open_ide: bool, environment_offscreen: bool) -> Self {
        Self {
            suppressed: if offscreen {
                Some("offscreen")
            } else if !open_ide {
                Some("disabled-for-session")
            } else if environment_offscreen {
                Some("offscreen-environment")
            } else {
                None
            },
        }
    }
}

impl IdeRequest {
    fn build(
        root: &Path,
        file: &Path,
        location: Option<(u32, u32)>,
        configured: &str,
    ) -> ara_ui::Result<Self> {
        if !root.is_absolute() || !file.is_absolute() {
            return Err("The IDE request requires absolute project and source paths".into());
        }
        let root = root.to_str().ok_or("The project path is not valid UTF-8")?;
        let file = file.to_str().ok_or("The source path is not valid UTF-8")?;
        let location = location.map(|(line, column)| (line.max(1), column.max(1)));
        if configured == "system" {
            return Ok(Self::system(root, file, location));
        }
        let program = if configured.is_empty() {
            if cfg!(windows) { "code.cmd" } else { "code" }
        } else {
            configured
        };
        let vscode = configured.is_empty() || is_vscode(program);
        let (args, line_handling) = match (vscode, location) {
            (true, Some((line, column))) => (
                vec![
                    "--reuse-window".into(),
                    "--goto".into(),
                    format!("{file}:{line}:{column}"),
                ],
                "vscode-goto",
            ),
            (true, None) => (
                vec!["--reuse-window".into(), root.into(), file.into()],
                "not-requested",
            ),
            (false, Some(_)) => (vec![file.into()], "unsupported-configured-editor"),
            (false, None) => (vec![root.into(), file.into()], "not-requested"),
        };
        let mut request = Self::invocation(root, file, location, program, args, line_handling);
        if configured.is_empty() {
            request.fallback = Some(Box::new(Self::system(root, file, location)));
        }
        Ok(request)
    }

    fn invocation(
        root: &str,
        file: &str,
        location: Option<(u32, u32)>,
        program: &str,
        args: Vec<String>,
        line_handling: &'static str,
    ) -> Self {
        Self {
            target_file: file.into(),
            line: location.map(|(line, _)| line),
            column: location.map(|(_, column)| column),
            program: program.into(),
            args,
            cwd: root.into(),
            line_handling,
            launch_suppressed: false,
            attempted: false,
            outcome: "prepared",
            suppression_reason: None,
            error: None,
            fallback: None,
        }
    }

    fn system(root: &str, file: &str, location: Option<(u32, u32)>) -> Self {
        let program = if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        Self::invocation(
            root,
            file,
            location,
            program,
            vec![file.into()],
            "unsupported-system-association",
        )
    }

    fn execute(
        &mut self,
        policy: LaunchPolicy,
        launch: &mut impl FnMut(&IdeRequest) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        if let Some(reason) = policy.suppressed {
            self.launch_suppressed = true;
            self.suppression_reason = Some(reason);
            self.outcome = "suppressed";
            if let Some(fallback) = &mut self.fallback {
                fallback.execute(policy, launch)?;
            }
            return Ok(());
        }
        self.attempted = true;
        match launch(self) {
            Ok(()) => {
                self.outcome = "launched";
                Ok(())
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.outcome = "failed";
                if let Some(fallback) = &mut self.fallback {
                    fallback.execute(policy, launch)?;
                    self.outcome = "fallback-launched";
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}

fn is_vscode(program: &str) -> bool {
    matches!(
        program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase()
            .as_str(),
        "code"
            | "code.cmd"
            | "code.exe"
            | "code-insiders"
            | "code-insiders.cmd"
            | "code-insiders.exe"
    )
}

fn spawn(request: &IdeRequest) -> std::io::Result<()> {
    let mut command = std::process::Command::new(&request.program);
    command.args(&request.args).current_dir(&request.cwd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.spawn().map(|_| ())
}

/// Reuse project path containment, including its explicit missing-relative-source contract.
pub(super) fn resolve_target(root: &Path, path: &Path) -> ara_ui::Result<(PathBuf, PathBuf)> {
    let root = std::fs::canonicalize(root)?;
    let file = if path.is_absolute() {
        std::fs::canonicalize(path)?
    } else {
        crate::commands::project_path(
            &root,
            path.to_str().ok_or("The source path is not valid UTF-8")?,
        )?
    };
    if !file.starts_with(&root) {
        return Err("This script points outside the open project".into());
    }
    Ok((root, file))
}

impl Shell {
    /// Inspect the exact last request without launching a process or changing its outcome.
    pub(crate) fn script_ide_request(&self) -> Value {
        self.ide_request
            .as_ref()
            .map_or(Value::Null, |request| json!(request))
    }

    /// Store complete argv first; hidden verification never reaches process creation.
    pub(in crate::shell::scripts) fn request_script_ide(
        &mut self,
        editor: &Editor,
        root: &Path,
        file: &Path,
        location: Option<(u32, u32)>,
    ) -> ara_ui::Result<()> {
        let configured = editor
            .state
            .settings
            .get("scripts.ide")
            .and_then(Value::as_str)
            .unwrap_or("");
        let request = IdeRequest::build(root, file, location, configured)?;
        let request = self.ide_request.insert(request);
        let policy = LaunchPolicy::new(
            self.offscreen,
            self.open_ide,
            std::env::var_os("ARA_PLAY_OFFSCREEN").is_some_and(|value| value == "1"),
        );
        request.execute(policy, &mut spawn)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "ide/tests.rs"]
mod tests;
