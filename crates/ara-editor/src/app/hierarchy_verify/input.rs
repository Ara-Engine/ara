//! Coordinate resolution uses the retained controls; ingress reuses the shared raw SDL helpers.
use super::*;
use crate::shell::Binding;

fn point(shell: &Shell, index: usize) -> Option<[f32; 2]> {
    if !shell.ui.shown(index) {
        return None;
    }
    let node = &shell.ui.nodes[index];
    let rect = node.rect.intersection(node.clip);
    (rect.w > 2. && rect.h > 2.).then_some([rect.x + rect.w * 0.5, rect.y + rect.h * 0.5])
}
pub(super) fn named(shell: &Shell, id: &str) -> Option<[f32; 2]> {
    point(shell, shell.shown_node(id)?)
}
pub(super) fn bound(shell: &Shell, predicate: impl Fn(&Binding, &str) -> bool) -> Option<[f32; 2]> {
    shell.bindings.iter().find_map(|(id, binding)| {
        let index = shell.ui.by_id(*id)?;
        predicate(binding, &shell.ui.nodes[index].element.id)
            .then(|| point(shell, index))
            .flatten()
    })
}
pub(super) fn row(shell: &Shell, entity: &str) -> Option<[f32; 2]> {
    bound(
        shell,
        |binding, _| matches!(binding,Binding::Select(id) if id==entity),
    )
}
pub(super) fn command(shell: &Shell, command: &str, palette: bool) -> Option<[f32; 2]> {
    bound(shell, |binding, id| {
        (if palette {
            id.starts_with("command-")
        } else {
            id.starts_with('m')
        }) && matches!(binding,Binding::Command(name,_) if *name==command)
    })
}
pub(super) fn submenu(shell: &Shell, label: &str) -> Option<[f32; 2]> {
    bound(shell, |binding, id| {
        matches!(binding, Binding::MenuSub(..))
            && shell
                .ui
                .find(id)
                .and_then(|i| shell.ui.nodes[i].element.attrs.get("label"))
                .is_some_and(|text| text == label)
    })
}
pub(super) fn dialog_field(shell: &Shell, title: &str) -> Option<String> {
    shell
        .ui
        .nodes
        .iter()
        .find(|node| {
            node.element.tag == "field"
                && node
                    .element
                    .attrs
                    .get("action")
                    .is_some_and(|action| action == "ara.dialog.text")
                && node
                    .element
                    .attrs
                    .get("label")
                    .is_some_and(|label| label == title)
                && shell.shown_node(&node.element.id).is_some()
        })
        .map(|node| node.element.id.clone())
}
pub(super) fn text_control(shell: &Shell, action: &str, text: &str) -> Option<[f32; 2]> {
    shell.ui.nodes.iter().enumerate().find_map(|(index, node)| {
        (node
            .element
            .attrs
            .get("action")
            .is_some_and(|value| value == action)
            && (node
                .element
                .attrs
                .get("text")
                .is_some_and(|value| value == text)
                || node.text.as_ref().is_some_and(|value| value.value == text)))
        .then(|| point(shell, index))
        .flatten()
    })
}

pub(super) fn click(
    window: &Window,
    at: [f32; 2],
    scale: f32,
    right: bool,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    native::motion(window, at, scale, events)?;
    if !right {
        native::button(window, true, events)?;
        return native::button(window, false, events);
    }
    use sdl3_sys::{events::*, video::SDL_WindowID};
    for down in [true, false] {
        let mut event = SDL_Event {
            button: SDL_MouseButtonEvent {
                r#type: if down {
                    SDL_EVENT_MOUSE_BUTTON_DOWN
                } else {
                    SDL_EVENT_MOUSE_BUTTON_UP
                },
                windowID: SDL_WindowID(window.id()),
                button: 3,
                down,
                clicks: 1,
                ..Default::default()
            },
        };
        // SAFETY: the selected SDL union member is fully initialized for a mouse-button event.
        if !unsafe { SDL_PushEvent(&mut event) } {
            return Err("SDL rejected hierarchy context input".into());
        }
        events.extend(Window::poll_tagged()?);
    }
    Ok(())
}

impl HierarchyCheck {
    pub(super) fn click_at(
        &mut self,
        window: &Window,
        at: Option<[f32; 2]>,
        scale: f32,
        right: bool,
        selector: &str,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        let Some(at) = at else {
            return Ok(false);
        };
        click(window, at, scale, right, events)?;
        self.trace.push(json!({"input":"native-pointer","selector":selector,"point":at,"button":if right{3}else{1}}));
        self.next();
        if right {
            self.pending_capture = Some("context-menu");
        }
        Ok(true)
    }
    pub(super) fn replace(
        &mut self,
        shell: &Shell,
        window: &Window,
        id: &str,
        text: &str,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        let Some(at) = named(shell, id) else {
            return Ok(false);
        };
        click(window, at, scale, false, events)?;
        native::shortcut(window, 4, events)?;
        native::tap(window, 42, events)?;
        if !text.is_empty() {
            native::text(window, text, events)?;
        }
        self.trace
            .push(json!({"input":"native-text","control":id,"value":text}));
        self.next();
        self.pending_capture = if id == "palette-search" {
            Some("palette")
        } else if id == "entity-rename-value" {
            Some("inline-rename")
        } else if id.starts_with("choice-") {
            Some("select-options")
        } else if shell.modal_active() {
            Some("prompt")
        } else {
            None
        };
        Ok(true)
    }
    pub(super) fn key(
        &mut self,
        window: &Window,
        scan: i32,
        shortcut: bool,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if shortcut {
            native::shortcut(window, scan, events)?;
        } else {
            native::tap(window, scan, events)?;
        }
        self.trace
            .push(json!({"input":"native-key","scancode":scan,"primary":shortcut}));
        self.next();
        Ok(())
    }
}
