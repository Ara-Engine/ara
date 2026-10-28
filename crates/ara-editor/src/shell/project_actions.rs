//! Explicit Hub handoff preserves this editor and records literal launch inputs.
use super::*;
use std::path::{Path, PathBuf};

/// Last requested native Hub invocation, retained even when policy suppresses it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchRequest {
    target: &'static str,
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    companions_available: bool,
    launch_suppressed: bool,
    attempted: bool,
    outcome: &'static str,
    error: Option<String>,
}

impl LaunchRequest {
    fn build(
        executable: &Path,
        config: &Path,
        project: &Path,
        new_project: bool,
    ) -> ara_ui::Result<Self> {
        let directory = executable
            .parent()
            .ok_or("The editor has no engine directory")?;
        let program = directory.join(format!("ara-hub{}", std::env::consts::EXE_SUFFIX));
        let cli = directory.join(format!("ara{}", std::env::consts::EXE_SUFFIX));
        let config = std::path::absolute(config)?;
        let utf8 = |path: &Path| {
            path.to_str()
                .map(str::to_owned)
                .ok_or("The Hub launch path is not valid UTF-8")
        };
        let mut args = vec![
            "--config-dir".into(),
            utf8(&config)?,
            "--ara".into(),
            utf8(&cli)?,
        ];
        if new_project {
            args.push("--new-project".into());
        }
        Ok(Self {
            target: if new_project {
                "newProject"
            } else {
                "projects"
            },
            companions_available: program.is_file() && cli.is_file(),
            program,
            args,
            cwd: project.to_path_buf(),
            launch_suppressed: false,
            attempted: false,
            outcome: "prepared",
            error: None,
        })
    }

    fn execute(
        &mut self,
        suppressed: bool,
        launch: &mut impl FnMut(&Self) -> std::io::Result<()>,
    ) -> ara_ui::Result<()> {
        if suppressed {
            self.launch_suppressed = true;
            self.outcome = "suppressed";
            return Ok(());
        }
        if !self.companions_available {
            self.outcome = "missing-companion";
            let message = "Ara Hub and its matching ara companion must be installed beside the editor. Repair this engine installation to open projects.";
            self.error = Some(message.into());
            return Err(message.into());
        }
        self.attempted = true;
        match launch(self) {
            Ok(()) => {
                self.outcome = "launched";
                Ok(())
            }
            Err(error) => {
                self.outcome = "failed";
                self.error = Some(error.to_string());
                Err(error.into())
            }
        }
    }
}

impl Shell {
    /// Hand off New/Open to the Hub's existing project and migration workflows.
    pub(crate) fn open_project_hub(
        &mut self,
        editor: &Editor,
        new_project: bool,
    ) -> ara_ui::Result<()> {
        let request = LaunchRequest::build(
            &std::env::current_exe()?,
            &crate::settings::config_dir()?,
            editor.host.root(),
            new_project,
        )?;
        let request = self.project_request.insert(request);
        let suppressed = self.offscreen
            || std::env::var_os("ARA_PLAY_OFFSCREEN").is_some_and(|value| value == "1");
        request.execute(suppressed, &mut |request| {
            let mut command = std::process::Command::new(&request.program);
            command.args(&request.args).current_dir(&request.cwd);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW suppresses a console, not the requested GUI.
            }
            command.spawn().map(|_| ())
        })?;
        self.status_note = Some(
            if suppressed {
                "Project Hub request recorded (offscreen)"
            } else if new_project {
                "Opened New Project in Ara Hub"
            } else {
                "Opened Projects in Ara Hub"
            }
            .into(),
        );
        Ok(())
    }

    /// Exact last request, without changing project state or starting a process.
    pub(crate) fn project_launch_request(&self) -> Value {
        self.project_request
            .as_ref()
            .map_or(Value::Null, |request| json!(request))
    }
}

#[cfg(test)]
#[path = "project_actions/tests.rs"]
mod tests;
