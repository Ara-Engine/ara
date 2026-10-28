use super::super::input_verify::native;
use super::*;
use crate::shell::Binding;

fn click(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let index = shell
        .shown_node(id)
        .ok_or_else(|| format!("Recovery control is not visible: {id}"))?;
    let rect = shell.ui.nodes[index]
        .rect
        .intersection(shell.ui.nodes[index].clip);
    if rect.w <= 0.5 || rect.h <= 0.5 {
        return Err(format!("Recovery control has no usable hit box: {id}").into());
    }
    native::motion(
        window,
        [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
        scale,
        events,
    )?;
    native::button(window, true, events)?;
    native::button(window, false, events)
}

impl BoundaryCheck {
    pub(super) fn input(
        &mut self,
        case: Case,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.age {
            3 => {
                shell.arm_panel_probe(editor, case.panel, case.kind, &self.correlation)?;
                shell.rebuild(editor)?;
            }
            6 => {
                let (point,_)=shell.bindings.iter().find_map(|(id,binding)| {
                    if !matches!(binding,Binding::Select(entity) if entity==B){return None;}
                    let index=shell.ui.by_id(*id).filter(|index|shell.ui.shown(*index))?;
                    let rect=shell.ui.nodes[index].rect.intersection(shell.ui.nodes[index].clip);
                    (rect.w>70. && rect.h>1.).then_some(([rect.x+64.,rect.y+rect.h*0.5],index))
                }).ok_or("Healthy hierarchy row is not reachable while the other panel is quarantined")?;
                native::motion(window, point, scale, events)?;
                native::button(window, true, events)?;
                native::button(window, false, events)?;
            }
            7 => click(shell, window, scale, "search-hierarchy", events)?,
            8 => {
                native::shortcut(window, 4, events)?;
                native::text(window, "Boundary recovery B", events)?;
            }
            10 => {
                let help = shell
                    .menu_header_node("Help")
                    .ok_or("Help menu is not registered")?;
                click(shell, window, scale, &help, events)?;
            }
            11 => native::tap(window, 81, events)?,
            13 => native::tap(window, 41, events)?,
            14 => click(
                shell,
                window,
                scale,
                &format!("panel-error-{}-retry", case.panel),
                events,
            )?,
            19 => click(shell, window, scale, "search-hierarchy", events)?,
            20 => {
                native::shortcut(window, 4, events)?;
                native::text(window, "Boundary recovery", events)?;
            }
            _ => {}
        }
        Ok(())
    }
}
