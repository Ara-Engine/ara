//! A failed panel is quarantined without discarding the other panels or their input.
use super::{Editor, Shell, escape};
use ara_ui::source::stable_id;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Controlled failure kind for the isolated, offscreen panel recovery journey.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ProbeFailure {
    /// Return an ordinary panel-render error after its real staged render.
    Error,
    /// Unwind inside the production panel boundary after its real staged render.
    Panic,
}

/// An explicitly armed one-shot probe; ordinary shells and soak processes keep `None`.
pub(super) struct Probe {
    panel: String,
    kind: ProbeFailure,
    correlation: String,
}
impl Probe {
    fn fail(self) -> ara_ui::Result<()> {
        let message = format!("Panel recovery verification {}", self.correlation);
        match self.kind {
            ProbeFailure::Error => Err(message.into()),
            ProbeFailure::Panic => panic!("{message}"),
        }
    }
}

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
    /// Arm a render failure only in the hidden native recovery fixture. This is an
    /// internal journey seam, never an editor command, registry entry or global flag.
    pub(crate) fn arm_panel_probe(
        &mut self,
        editor: &Editor,
        panel: &str,
        kind: ProbeFailure,
        correlation: &str,
    ) -> ara_ui::Result<()> {
        if !self.offscreen || !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err(
                "Panel recovery probes require the isolated offscreen behaviour fixture".into(),
            );
        }
        if self.panel_probe.is_some() || self.panel_failures.contains_key(panel) {
            return Err(
                "Retry the current failed panel before arming another recovery probe".into(),
            );
        }
        if self.shown_node(&format!("body-{panel}")).is_none()
            || correlation.is_empty()
            || correlation.len() > 120
        {
            return Err(
                "Recovery probe requires a mounted visible panel and a bounded correlation label"
                    .into(),
            );
        }
        // The journey exercises the real generator even for memoized Assets.
        self.memos.remove(panel);
        self.panel_probe = Some(Probe {
            panel: panel.to_owned(),
            kind,
            correlation: correlation.to_owned(),
        });
        Ok(())
    }

    /// End the recovery story without retaining fault injection into another journey.
    pub(crate) fn disarm_panel_probe(&mut self) {
        self.panel_probe = None;
    }

    /// Whether the next matching real render still has an armed verification failure.
    pub(crate) fn panel_probe_armed(&self) -> bool {
        self.panel_probe.is_some()
    }

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
                stable_id(&format!("panel-error-{panel}-retry")),
                super::Binding::PanelRetry(panel.to_owned()),
            );
            return;
        }

        // Panel renderers register their own bindings. Keep earlier panels' bindings
        // outside the unwind so a half-built panel cannot retain clickable controls.
        let previous = std::mem::take(&mut self.bindings);
        let mut body = String::new();
        let result = catch_unwind(AssertUnwindSafe(|| -> ara_ui::Result<()> {
            render(self, editor, &mut body)?;
            if let Some(probe) = self.panel_probe.take_if(|probe| probe.panel == panel)
                && self.offscreen
            {
                probe.fail()?;
            }
            Ok(())
        }));
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
            stable_id(&format!("panel-error-{panel}-retry")),
            super::Binding::PanelRetry(panel.to_owned()),
        );
        self.panel_failures.insert(panel.to_owned(), failure);
    }
}

#[cfg(test)]
#[path = "panel_boundary/probe_tests.rs"]
mod probe_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
