//! A failed panel is quarantined without discarding the other panels or their input.
use super::{Editor, Shell, escape};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The first failed render of a panel, retained until the user retries it.
#[derive(Clone, Debug)]
pub(super) struct Failure {
    message: String,
    panicked: bool,
}

impl Failure {
    fn error(error: impl std::fmt::Display) -> Self {
        Self {
            message: error.to_string(),
            panicked: false,
        }
    }

    fn panic(payload: Box<dyn std::any::Any + Send>) -> Self {
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("panel panicked with a non-text payload");
        Self {
            message: message.to_owned(),
            panicked: true,
        }
    }

    fn markup(&self, panel: &str) -> String {
        let id = escape(panel);
        format!(
            "<panel id='panel-error-{id}' class='empty'><text id='panel-error-{id}-title' class='empty-title' text='This panel could not be displayed' /><text id='panel-error-{id}-detail' class='empty-hint' text='{}' /><text id='panel-error-{id}-help' class='empty-hint' text='The other panels are still available. Open Console for details, then retry this panel.' /><button id='panel-error-{id}-retry' text='Retry panel' action='Dispatch' /></panel>",
            escape(&self.message)
        )
    }
}

impl Shell {
    /// Number of quarantined panel renders for diagnostics and bounded journeys.
    pub(crate) fn panel_failure_count(&self) -> usize {
        self.panel_failures.len()
    }

    /// Clear a panel's quarantine before retrying its normal retained render.
    pub(crate) fn retry_panel(&mut self, panel: &str) -> bool {
        self.bodies.remove(panel);
        self.panel_failures.remove(panel).is_some()
    }

    /// Build markup and bindings together. A failure publishes neither partial output.
    pub(super) fn with_panel_boundary(
        &mut self,
        panel: &str,
        editor: &mut Editor,
        out: &mut String,
        render: impl FnOnce(&mut Shell, &mut Editor, &mut String) -> ara_ui::Result<()>,
    ) {
        if let Some(failure) = self.panel_failures.get(panel) {
            out.push_str(&failure.markup(panel));
            self.bindings.insert(
                super::stable_id(&format!("panel-error-{panel}-retry")),
                super::Binding::PanelRetry(panel.to_owned()),
            );
            return;
        }

        // Panel renderers register their own bindings. Keep earlier panels' bindings
        // outside the unwind so a half-built panel cannot retain clickable controls.
        let previous = std::mem::take(&mut self.bindings);
        let mut body = String::new();
        let result = catch_unwind(AssertUnwindSafe(|| render(self, editor, &mut body)));
        let staged = std::mem::replace(&mut self.bindings, previous);
        let failure = match result {
            Ok(Ok(())) => {
                self.bindings.extend(staged);
                out.push_str(&body);
                return;
            }
            Ok(Err(error)) => Failure::error(error),
            Err(payload) => Failure::panic(payload),
        };
        editor.report(
            "action",
            &format!(
                "{} panel {}: {}",
                super::panels::title(panel),
                if failure.panicked {
                    "panicked"
                } else {
                    "failed"
                },
                failure.message
            ),
            Some(panel),
        );
        out.push_str(&failure.markup(panel));
        self.bindings.insert(
            super::stable_id(&format!("panel-error-{panel}-retry")),
            super::Binding::PanelRetry(panel.to_owned()),
        );
        self.panel_failures.insert(panel.to_owned(), failure);
    }
}

#[cfg(test)]
mod tests;
