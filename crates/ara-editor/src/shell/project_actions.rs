//! The old window's Hub hand-off ([`crate::hub_launch`]) and its last request.
use super::*;

impl Shell {
    /// Hand off New/Open to the Hub's existing project and migration workflows.
    pub(crate) fn open_project_hub(
        &mut self,
        editor: &Editor,
        new_project: bool,
    ) -> ara_ui::Result<()> {
        let suppressed = self.offscreen
            || std::env::var_os("ARA_PLAY_OFFSCREEN").is_some_and(|value| value == "1");
        let request = crate::hub_launch::open(editor.host.root(), new_project, suppressed)?;
        let failed = request.error.clone();
        self.project_request = Some(request);
        if let Some(error) = failed {
            return Err(error.into());
        }
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
