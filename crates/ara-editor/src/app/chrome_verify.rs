//! Offscreen checks of titlebar geometry, command routes and Windows nonclient hit testing.
use super::*;
pub(super) fn verify(
    editor: &mut Editor,
    shell: &mut Shell,
    window: &mut Window,
    snap: &mut window_chrome::SnapCaption,
    output: &std::path::Path,
    dpi: f32,
) -> ara_ui::Result<()> {
    editor.action("editor.layout.reset", json!({}))?;
    shell.rebuild(editor)?;
    shell.frame()?;
    if let Some(button) = shell.update_chrome(window, dpi) {
        snap.update(button);
    }
    let rect = |shell: &Shell, id: &str| -> ara_ui::Result<ara_ui::paint::Box2> {
        Ok(shell.ui.nodes[shell
            .shown_node(id)
            .ok_or_else(|| format!("Missing chrome control {id}"))?]
        .rect)
    };
    let [left, view, bottom, right]: [ara_ui::paint::Box2; 4] =
        ["area-outline", "area-view", "area-bottom", "area-right"]
            .map(|id| rect(shell, id))
            .into_iter()
            .collect::<ara_ui::Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| "Four default areas")?;
    if (left.y - right.y).abs() > 1.
        || (left.h - right.h).abs() > 1.
        || bottom.x < left.x + left.w
        || bottom.x + bottom.w > right.x + 1.
        || (view.x - bottom.x).abs() > 1.
        || (view.w - bottom.w).abs() > 1.
    {
        return Err("Default layout must keep the bottom dock between full-height sidebars".into());
    }
    let controls = rect(shell, "play-controls")?;
    if (controls.x + controls.w * 0.5 - shell.ui.size[0] * 0.5).abs() > 1. {
        return Err("Play controls are not centered on the window".into());
    }
    let mut clipboard = OsClipboard;
    shell.click("brand", &mut clipboard)?;
    shell.dispatch(editor)?;
    shell.rebuild(editor)?;
    let version = shell
        .ui
        .find("dialog-line-0")
        .and_then(|i| shell.ui.nodes[i].text.as_ref())
        .map(|t| t.value.as_str());
    let expected_version = format!("Ara {}", env!("CARGO_PKG_VERSION"));
    if version != Some(expected_version.as_str()) {
        return Err("Logo must open About with the release version".into());
    }
    shell.click("dialog-close", &mut clipboard)?;
    shell.dispatch(editor)?;
    shell.rebuild(editor)?;
    for (id, command) in [
        ("window-minimize", "window.minimize"),
        ("window-maximize", "window.maximize_window"),
        ("window-close", "file.exit"),
    ] {
        if !shell
            .command_bindings()
            .iter()
            .any(|(i, c)| i == id && *c == command)
        {
            return Err(format!("{id} is not bound to {command}").into());
        }
    }
    for (id, request) in [
        ("window-minimize", crate::shell::WindowRequest::Minimize),
        ("window-maximize", crate::shell::WindowRequest::Maximize),
        ("window-close", crate::shell::WindowRequest::Quit),
    ] {
        shell.click(id, &mut clipboard)?;
        shell.dispatch(editor)?;
        if shell.window_request.take() != Some(request) {
            return Err(
                format!("{id} did not request its native action on the first click").into(),
            );
        }
    }
    shell.pointer = [-1., -1.];
    shell.event(InputEvent::PointerMove(shell.pointer), &mut clipboard)?;
    shell.input.blur(&mut shell.ui);
    shell.rebuild(editor)?;
    shell.frame()?;
    if let Some(button) = shell.update_chrome(window, dpi) {
        snap.update(button);
    }
    let native = snap.verify(window, shell, dpi)?;
    std::fs::create_dir_all(output)?;
    std::fs::write(
        output.join("chrome.json"),
        ara_json::to_canonical_string(
            &json!({"verified":true,"fullHeightSidebars":true,"centerBottomDock":true,"centeredPlay":true,"logoAbout":true,"windowCommands":true,"nativeHitTest":native}),
        ),
    )?;
    Ok(())
}
