//! Native evidence uses the same geometry checks as the full-editor quality sweep.
use super::*;

impl Shell {
    pub(crate) fn audit_feedback_capture(
        &self,
        output: &std::path::Path,
        prefix: &str,
    ) -> ara_ui::Result<()> {
        let audit = feedback_geometry::audit(&self.ui);
        std::fs::write(
            output.join(format!("{prefix}.feedback.json")),
            ara_json::to_canonical_string(&json!({
                "surfaces": audit.surfaces,
                "violations": audit.violations,
                "logicalViewport": self.ui.size,
            })),
        )?;
        if !audit.violations.is_empty() {
            return Err(format!(
                "Feedback geometry failed for {prefix}: {}",
                audit.violations.join("; ")
            )
            .into());
        }
        Ok(())
    }
}
