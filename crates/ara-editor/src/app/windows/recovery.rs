//! A failed native window must not take down its panels or the editor authority.
use super::*;

/// Return only the failed host's tabs to the main view through navigation ops.
pub(super) fn redock(
    editor: &mut Editor,
    shell: &mut Shell,
    spec: &ara_ui::docking::Floating,
    error: &(dyn std::error::Error + 'static),
) -> ara_ui::Result<()> {
    fn panels(node: &ara_ui::docking::DockNode, out: &mut Vec<String>) {
        match node {
            ara_ui::docking::DockNode::Area { tabs, .. } => out.extend(tabs.iter().cloned()),
            ara_ui::docking::DockNode::Split { first, second, .. } => {
                panels(first, out);
                panels(second, out);
            }
        }
    }
    let mut tabs = Vec::new();
    panels(&spec.root, &mut tabs);
    shell.leave();
    shell.drag = None;
    shell.drop_preview = None;
    shell.cancel_pointer_capture();
    for panel in &tabs {
        editor.action("editor.layout.move", json!({"panel":panel,"slot":"view"}))?;
    }
    let message = format!(
        "Could not open floating window {} ({:?}): {error}. Its panels were returned to the main window; Move Panel to New Window retries.",
        spec.id, spec.rect,
    );
    editor.report_failure(&std::io::Error::other(message.clone()));
    shell.status_note = Some(message);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
