use super::*;
use crate::shell::{Binding, commands::Target};

fn point(shell: &Shell, index: usize) -> Option<[f32; 2]> {
    if !shell.ui.shown(index) {
        return None;
    }
    let node = &shell.ui.nodes[index];
    let x = node.rect.x.max(node.clip.x);
    let y = node.rect.y.max(node.clip.y);
    let right = (node.rect.x + node.rect.w).min(node.clip.x + node.clip.w);
    let bottom = (node.rect.y + node.rect.h).min(node.clip.y + node.clip.h);
    (right - x > 2. && bottom - y > 2.).then_some([(x + right) * 0.5, (y + bottom) * 0.5])
}
fn named(shell: &Shell, name: &str) -> Option<[f32; 2]> {
    point(shell, shell.shown_node(name)?)
}
fn bound(shell: &Shell, predicate: impl Fn(&Binding, &str) -> bool) -> Option<[f32; 2]> {
    shell.bindings.iter().find_map(|(id, binding)| {
        let index = shell.ui.by_id(*id)?;
        predicate(binding, &shell.ui.nodes[index].element.id)
            .then(|| point(shell, index))
            .flatten()
    })
}
fn submenu(shell: &Shell, label: &str) -> Option<[f32; 2]> {
    bound(shell, |binding, id| {
        matches!(binding, Binding::MenuSub(..))
            && shell
                .ui
                .find(id)
                .and_then(|index| shell.ui.nodes[index].element.attrs.get("label"))
                .is_some_and(|title| title == label)
    })
}
fn destination(target: &Target) -> Option<String> {
    let Target::Text(text) = target else {
        return None;
    };
    if text.starts_with('@') {
        Some(text.clone())
    } else {
        serde_json::from_str::<Value>(text).ok()?["destination"]
            .as_str()
            .map(str::to_owned)
    }
}
fn motion(window: &Window, at: [f32; 2], scale: f32, events: &mut Vec<(u32, Event)>) {
    events.push((
        window.id(),
        Event::MouseMotion {
            position: at.map(|value| value / scale),
            delta: [0.; 2],
        },
    ));
}
fn click(window: &Window, at: [f32; 2], scale: f32, button: u8, events: &mut Vec<(u32, Event)>) {
    motion(window, at, scale, events);
    for pressed in [true, false] {
        events.push((window.id(), Event::MouseButton { button, pressed }));
    }
}
fn key(window: &Window, code: i32, pressed: bool, events: &mut Vec<(u32, Event)>) {
    events.push((
        window.id(),
        Event::Key {
            scancode: code,
            pressed,
            repeat: false,
        },
    ));
}
impl PrefabCheck {
    fn wait(&mut self, selector: &str) -> ara_ui::Result<()> {
        if self.waited >= PATIENCE {
            return Err(format!("J5 native control did not become reachable: {selector}").into());
        }
        self.waiting = true;
        Ok(())
    }
    pub(super) fn input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !scale.is_finite() || scale <= 0. {
            return Err("J5 input scale is invalid".into());
        }
        if self.age == 0 {
            events.push((window.id(), Event::FocusLost));
            return self.setup(act, editor, shell);
        }
        if self.age == 6 {
            let (selector, position, button) = match act {
                Act::NestedExplicit => (
                    "Inspector Transform.scale context",
                    named(shell, "field-Transform-scale"),
                    3,
                ),
                Act::VariantCreate => (
                    "Hierarchy variant root context",
                    bound(
                        shell,
                        |binding, _| matches!(binding,Binding::Select(entity) if entity==VARIANT_ENTITY),
                    ),
                    3,
                ),
                _ => ("Inspector Apply all", named(shell, "prefab-apply"), 1),
            };
            let Some(position) = position else {
                return self.wait(selector);
            };
            click(window, position, scale, button, events);
            self.trace.push(json!({"input":"pointer-click","selector":selector,"point":position,"button":button}));
        } else if self.age == 9 && act == Act::NestedExplicit {
            let Some(position) = submenu(shell, "Apply To") else {
                return self.wait("Field > Apply To submenu");
            };
            motion(window, position, scale, events);
            self.trace.push(
                json!({"input":"pointer-hover","selector":"Field > Apply To","point":position}),
            );
        } else if self.age == 12 && act == Act::NestedExplicit {
            let wanted = "@prefabs/j5-base.prefab#pf_j5base";
            let Some(position) = bound(shell, |binding, id| {
                id.starts_with('m')
                    && matches!(binding,
                Binding::Command("prefab.apply_to",target) if destination(target).as_deref()==Some(wanted))
            }) else {
                return self.wait("Apply To > j5-base.prefab");
            };
            click(window, position, scale, 1, events);
            self.trace.push(json!({"input":"pointer-click","selector":"prefab.apply_to","destination":wanted,"point":position}));
        } else if self.age == 12 && act == Act::VariantCreate {
            let Some(position) = bound(shell, |binding, id| {
                id.starts_with('m') && matches!(binding, Binding::Command("create.prefab", _))
            }) else {
                return self.wait("Entity > Prefab from Selection");
            };
            click(window, position, scale, 1, events);
            self.trace
                .push(json!({"input":"pointer-click","selector":"create.prefab","point":position}));
        } else if self.age == 20 && act != Act::FailedApply {
            let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
            key(window, primary, true, events);
            key(window, 29, true, events);
            key(window, 29, false, events);
            key(window, primary, false, events);
            self.trace.push(json!({"input":"shortcut","keys":"Ctrl+Z"}));
        } else if self.age == 24 && act == Act::FailedApply {
            self.diagnostics = editor.action(
                "logs",
                json!({"since":self.log_cursor,"severity":"error","group":false,"limit":100}),
            )?;
        }
        Ok(())
    }
}
