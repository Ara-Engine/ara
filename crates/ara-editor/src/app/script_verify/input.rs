//! Every measured control goes through SDL and the production event owner.
use super::super::input_verify::native;
use super::*;
use crate::shell::Binding;
mod create;
mod history;
mod ide;
mod modules;
mod problems;

impl ScriptCheck {
    pub(super) fn input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.started.elapsed() > PATIENCE {
            return Err(format!(
                "Native J10 {} phase {} exceeded {}s: {}",
                CASES[self.index],
                self.phase,
                PATIENCE.as_secs(),
                self.snapshot(editor, shell)
            )
            .into());
        }
        if self.phase == 0 {
            self.pending_capture = Some("before");
            self.next();
            return Ok(());
        }
        match self.index {
            0 => self.create(editor, shell, window, scale, events)?,
            1 => self.modules(editor, shell, window, scale, events)?,
            2 => {
                if self.phase == 1 {
                    self.click(shell, window, scale, "play", events)?;
                    self.next();
                } else if shell.game_running {
                    let world = editor
                        .play_world
                        .clone()
                        .ok_or("Native Play reports running without a world")?;
                    self.world = Some(world.clone());
                    self.done(
                        json!({"world":world,"running":true,"ingress":"SDL toolbar pointer"}),
                    );
                }
            }
            3 | 4 => {
                let stage = if self.index == 3 {
                    Stage::Changed
                } else {
                    Stage::Broken
                };
                if self
                    .proof
                    .as_mut()
                    .ok_or("Native J10 reload proof is missing")?
                    .run(stage, editor)?
                {
                    let proof = self
                        .proof
                        .as_ref()
                        .ok_or("Native J10 proof disappeared")?
                        .report();
                    self.done(json!({"runtimeEffect":proof,"ingress":"Protocol source.replace jobs and background ECS reads; not native source editing",
                        "currentProblemsCleared":Value::Null,"currentProblemsScope":"Compile projection recorded separately; module Reload.errors alone is not a Problems retirement assertion"}));
                }
            }
            5 => self.problems(editor, shell, window, scale, events)?,
            6 => self.history(editor, shell, window, scale, events)?,
            7 => {
                if self.phase == 1 {
                    self.click(shell, window, scale, "stop", events)?;
                    self.next();
                } else if editor.play_world.is_none() && !shell.game_running && !shell.game_live {
                    let status = editor.action("play.status", json!({}))?;
                    let still_owned = status["worlds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .chain(status["stopping"].as_array().into_iter().flatten())
                        .any(|world| world["world"].as_str() == self.world.as_deref());
                    if !still_owned {
                        self.pointer = None;
                        shell.desktop_pointer = None;
                        self.done(json!({"worldRetired":self.world,"status":status,"ingress":"SDL toolbar pointer; read-only retirement observation"}));
                    }
                }
            }
            _ => return Err("Native J10 case is out of range".into()),
        }
        Ok(())
    }
    fn palette(
        &mut self,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        command: &str,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        match self.phase {
            1 => {
                native::shortcut(window, 14, events)?;
                self.next();
            }
            2 => {
                if shell.shown_node("palette-search").is_none() {
                    return Ok(false);
                }
                native::shortcut(window, 4, events)?;
                native::text(window, command, events)?;
                self.next();
            }
            3 => {
                let id = shell
                    .command_bindings()
                    .into_iter()
                    .find(|(id, binding)| {
                        id.starts_with("command-") && binding == command && visible(shell, id)
                    })
                    .map(|(id, _)| id);
                if let Some(id) = id {
                    self.click(shell, window, scale, &id, events)?;
                    self.next();
                }
            }
            _ => return Ok(true),
        }
        Ok(false)
    }
    fn click(
        &mut self,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        id: &str,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.trace.len() >= 256 {
            return Err("Native J10 bounded input trace exhausted".into());
        }
        let index = shell
            .shown_node(id)
            .ok_or_else(|| format!("Native J10 control is not shown: {id}"))?;
        let node = &shell.ui.nodes[index];
        let rect = node.rect.intersection(node.clip);
        if rect.w <= 1. || rect.h < node.rect.h * 0.75 {
            return Err(format!("Native J10 control is clipped: {id}").into());
        }
        let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        let desktop = window.rect()?;
        self.pointer = Some([
            desktop[0] as f32 + point[0] / scale,
            desktop[1] as f32 + point[1] / scale,
        ]);
        shell.desktop_pointer = self.pointer;
        self.trace.push(json!({"journey":CASES[self.index],"phase":self.phase,"ingress":"SDL pointer","control":id,
            "logical":point,"desktop":self.pointer,"pointsToLogical":scale,"bounds":[rect.x,rect.y,rect.w,rect.h]}));
        native::motion(window, point, scale, events)?;
        native::button(window, true, events)?;
        native::button(window, false, events)
    }
    fn reveal(
        &mut self,
        shell: &Shell,
        window: &Window,
        scale: f32,
        id: &str,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        if visible(shell, id) {
            return Ok(true);
        }
        let Some(index) = shell.ui.find(id) else {
            return Ok(false);
        };
        let node = &shell.ui.nodes[index];
        let clip = node.clip;
        if clip.w > 1. && clip.h > 1. {
            native::motion(
                window,
                [clip.x + clip.w * 0.5, clip.y + clip.h * 0.5],
                scale,
                events,
            )?;
            native::wheel(window, if node.rect.y < clip.y { 3. } else { -3. }, events)?;
        }
        Ok(false)
    }
}
fn visible(shell: &Shell, id: &str) -> bool {
    shell.shown_node(id).is_some_and(|index| {
        let node = &shell.ui.nodes[index];
        let rect = node.rect.intersection(node.clip);
        rect.w > 1. && rect.h >= node.rect.h * 0.75
    })
}
