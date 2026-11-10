//! Opening Ara Hub from the editor (File › New Project, Open Project): the Hub's own
//! project and migration workflows take over while this editor stays open. The exact
//! request is kept, also when policy suppresses the launch (offscreen runs).
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
    pub(crate) error: Option<String>,
}

impl LaunchRequest {
    pub(crate) fn build(
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

    pub(crate) fn execute(
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

/// Opens Ara Hub on its Projects page, or its New Project wizard, for `project`'s engine;
/// with `suppressed` (offscreen runs) only records the request. The request comes back
/// either way, its outcome and any error recorded; only preparing it can fail.
pub(crate) fn open(
    project: &Path,
    new_project: bool,
    suppressed: bool,
) -> ara_ui::Result<LaunchRequest> {
    let mut request = LaunchRequest::build(
        &std::env::current_exe()?,
        &crate::settings::config_dir()?,
        project,
        new_project,
    )?;
    // A failed launch is in the request (`outcome`, `error`).
    let _ = request.execute(suppressed, &mut |request| {
        let mut command = std::process::Command::new(&request.program);
        command.args(&request.args).current_dir(&request.cwd);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW suppresses a console, not the requested GUI.
        }
        command.spawn().map(|_| ())
    });
    Ok(request)
}

#[cfg(test)]
mod tests;
