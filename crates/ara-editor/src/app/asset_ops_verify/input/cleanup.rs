//! Dismiss completed results through visible native controls before a new baseline.
use super::*;

impl AssetOpsCheck {
    pub(super) fn dismiss_terminal_results(
        &mut self,
        shell: &Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        let target = shell
            .ui
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                if !shell.ui.shown(index) {
                    return None;
                }
                let command = match shell
                    .bindings
                    .get(&ara_ui::source::stable_id(&node.element.id))
                {
                    Some(Binding::Command(command, _))
                        if matches!(
                            *command,
                            "assets.operation.dismiss" | "assets.import.dismiss"
                        ) =>
                    {
                        *command
                    }
                    _ => return None,
                };
                Some((command, node.element.id.as_str(), node))
            })
            .min_by_key(|(command, _, _)| usize::from(*command != "assets.operation.dismiss"));
        let Some((command, id, node)) = target else {
            return Ok(true);
        };
        let visible = node.rect.intersection(node.clip);
        let fully_visible = !node.state.disabled
            && node.rect.w > 2.
            && node.rect.h > 2.
            && visible.w >= node.rect.w * 0.99
            && visible.h >= node.rect.h * 0.99;
        if !fully_visible {
            if command == "assets.import.dismiss" {
                let scroll = shell
                    .shown_node("asset-jobs-scroll")
                    .ok_or("Completed import result has no native scroll viewport")?;
                let viewport = shell.ui.nodes[scroll]
                    .rect
                    .intersection(shell.ui.nodes[scroll].clip);
                if viewport.w <= 2. || viewport.h <= 2. {
                    return Err(
                        "Completed import Dismiss is clipped with no reachable scroll viewport"
                            .into(),
                    );
                }
                self.motion(
                    window,
                    [viewport.x + viewport.w * 0.5, viewport.y + viewport.h * 0.5],
                    scale,
                    events,
                );
                events.push((
                    window.id(),
                    Event::Wheel([
                        0.,
                        if node.rect.y < viewport.y {
                            0.125
                        } else {
                            -0.125
                        },
                    ]),
                ));
                return Ok(false);
            }
            return Err("Completed asset result Dismiss is not fully reachable".into());
        }
        self.trace.push(json!({"stage":"setup-native-dismiss","age":self.age,"command":command,"id":id,"rect":node.rect,"clip":node.clip,"fullyVisible":true,"label":node.element.attrs.get("label")}));
        self.click(shell, window, id, scale, events)?;
        Ok(false)
    }
}
