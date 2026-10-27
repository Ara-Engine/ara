//! Replayable actions and a fixed integer RNG; elapsed time never selects an action.
use crate::{
    Editor,
    shell::{
        Shell,
        commands::{self, Target},
    },
};
use ara_ui::input::InputEvent;
use ara_ui_text::{Clipboard, EditEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;

const TARGET_CASES: usize = 12;
const PARAM_CASES: usize = 5;

#[derive(Default)]
pub(super) struct MemoryClipboard(String);
impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.to_owned();
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum Action {
    Command {
        id: String,
        target: Target,
    },
    Protocol {
        method: String,
        params: Value,
        raw: bool,
    },
    Click {
        point: [f32; 2],
    },
    Move {
        point: [f32; 2],
    },
    Press {
        point: [f32; 2],
    },
    Release {
        point: [f32; 2],
        cancel: bool,
    },
    Wheel {
        point: [f32; 2],
        delta: [f32; 2],
    },
    Tab {
        reverse: bool,
    },
    Key {
        scancode: i32,
    },
    Text {
        id: Option<String>,
        value: String,
    },
    FloatingInput {
        surface: String,
        reverse: bool,
    },
}

impl Action {
    pub fn run(
        &self,
        editor: &mut Editor,
        shell: &mut Shell,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<()> {
        match self {
            Self::Command { id, target } => shell.run_command(editor, id, target),
            Self::Protocol {
                method,
                params,
                raw,
            } => {
                if *raw {
                    editor
                        .call_for(ara_proto::ConnId::LOCAL, method, params.clone())
                        .map_err(crate::EditorError::from)?;
                } else {
                    editor.action(method, params.clone())?;
                }
                shell.rebuild(editor)
            }
            Self::Click { point } => {
                shell.pointer = *point;
                shell.event(InputEvent::PointerDown(*point), clipboard)?;
                shell.event(InputEvent::PointerUp(*point), clipboard)?;
                shell.dispatch(editor)?;
                Ok(())
            }
            Self::Move { point } => {
                shell.pointer = *point;
                shell.event(InputEvent::PointerMove(*point), clipboard)?;
                if shell.content_pending() {
                    shell.content_drag_over(editor, 0.016)?;
                }
                shell.drag_over();
                Ok(())
            }
            Self::Press { point } => {
                shell.pointer = *point;
                shell.event(InputEvent::PointerDown(*point), clipboard)?;
                shell.begin_drag();
                Ok(())
            }
            Self::Release { point, cancel } => {
                shell.pointer = *point;
                if *cancel {
                    shell.key(editor, 41, true, clipboard)?;
                    shell.key(editor, 41, false, clipboard)?;
                } else {
                    shell.drop_at(editor)?;
                    shell.event(InputEvent::PointerUp(*point), clipboard)?;
                    shell.dispatch(editor)?;
                }
                Ok(())
            }
            Self::Wheel { point, delta } => {
                shell.pointer = *point;
                shell.event(
                    InputEvent::Wheel {
                        position: *point,
                        delta: *delta,
                    },
                    clipboard,
                )?;
                shell.dispatch(editor)?;
                Ok(())
            }
            Self::Tab { reverse } => shell.event(InputEvent::Tab { reverse: *reverse }, clipboard),
            Self::Key { scancode } => {
                shell.key(editor, *scancode, true, clipboard)?;
                shell.key(editor, *scancode, false, clipboard)?;
                shell.dispatch(editor)?;
                Ok(())
            }
            Self::Text { id, value } => {
                if let Some(id) = id {
                    shell.click(id, clipboard)?;
                    shell.dispatch(editor)?;
                }
                shell.event(InputEvent::Edit(EditEvent::SelectAll), clipboard)?;
                shell.event(
                    InputEvent::Edit(EditEvent::Insert(value.clone())),
                    clipboard,
                )?;
                shell.dispatch(editor)?;
                shell.key(editor, 40, true, clipboard)?;
                shell.key(editor, 40, false, clipboard)?;
                Ok(())
            }
            Self::FloatingInput { surface, reverse } => {
                if !shell.enter(surface) {
                    return Err("floating surface closed before its input arrived".into());
                }
                let result = (|| {
                    shell.event(InputEvent::Tab { reverse: *reverse }, clipboard)?;
                    shell.event(InputEvent::Activate, clipboard)?;
                    shell.dispatch(editor)?;
                    Ok(())
                })();
                shell.leave();
                result
            }
        }
    }
}

pub(super) struct Generator {
    rng: u64,
    command_cursor: usize,
    method_cursor: usize,
    methods: Vec<String>,
    initialized: bool,
    pending: VecDeque<Action>,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: seed,
            command_cursor: 0,
            method_cursor: 0,
            methods: Vec::new(),
            initialized: false,
            pending: VecDeque::new(),
        }
    }

    pub fn initialize(&mut self, editor: &Editor) {
        if !self.initialized {
            self.methods = editor
                .host
                .catalog()
                .iter()
                .map(|method| method.name.clone())
                .collect();
            self.initialized = true;
        }
    }

    /// SplitMix64 has a defined stream for every seed, including zero.
    fn random(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.rng;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }

    fn index(&mut self, count: usize) -> usize {
        if count == 0 {
            0
        } else {
            (self.random() % count as u64) as usize
        }
    }

    pub fn next(&mut self, shell: &Shell) -> Action {
        if let Some(action) = self.pending.pop_front() {
            return action;
        }
        // Complete deterministic command coverage before the unbounded random soak.
        let specs = commands::all();
        if self.command_cursor < specs.len() * TARGET_CASES {
            let spec = &specs[self.command_cursor / TARGET_CASES];
            let target = edge_target(self.command_cursor % TARGET_CASES);
            self.command_cursor += 1;
            return registry_action(spec.id, target);
        }
        if self.method_cursor < self.methods.len() * PARAM_CASES {
            let method = self.methods[self.method_cursor / PARAM_CASES].clone();
            let params = edge_params(self.method_cursor % PARAM_CASES);
            self.method_cursor += 1;
            return Action::Protocol {
                method,
                params,
                raw: true,
            };
        }
        let [width, height] = shell.ui.size;
        let point = [
            self.index(width.max(1.) as usize) as f32,
            self.index(height.max(1.) as usize) as f32,
        ];
        match self.index(14) {
            0 => Action::Click { point },
            1 => Action::Move {
                point: [point[0] - width * 0.2, point[1] - height * 0.2],
            },
            2 => Action::Tab {
                reverse: self.index(2) == 0,
            },
            3 => {
                const KEYS: [i32; 9] = [41, 40, 43, 44, 76, 73, 74, 77, 67];
                Action::Key {
                    scancode: KEYS[self.index(KEYS.len())],
                }
            }
            4 => Action::Wheel {
                point,
                delta: [0., [-1_000_000., -96., 96., 1_000_000.][self.index(4)]],
            },
            5 => {
                let fields: Vec<_> = shell
                    .ui
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(i, n)| {
                        shell.ui.shown(*i) && matches!(n.element.tag.as_str(), "field" | "input")
                    })
                    .map(|(_, n)| n.element.id.clone())
                    .collect();
                let id = fields.get(self.index(fields.len())).cloned();
                const VALUES: [&str; 8] = [
                    "",
                    "NaN",
                    "Infinity",
                    "-Infinity",
                    "1e308",
                    "-1e308",
                    "0",
                    "<>&'\"🧪",
                ];
                Action::Text {
                    id,
                    value: VALUES[self.index(VALUES.len())].into(),
                }
            }
            6 => {
                let spec = &specs[self.index(specs.len())];
                registry_action(spec.id, edge_target(self.index(TARGET_CASES)))
            }
            7 => {
                let (method, params) = match self.index(6) {
                    0 => (
                        "editor.select",
                        json!({"entities":["@levels/deleted.scene#e_deleted1", ""]}),
                    ),
                    1 => ("editor.camera", json!({"fovYDegrees":1e308})),
                    2 => ("editor.camera", json!({"position":[1e308,-1e308,0]})),
                    3 => (
                        "editor.layout",
                        json!({"path":[true,true,true,true],"ratio":-1e308}),
                    ),
                    4 => ("editor.scene.open", json!({"scene":"levels/deleted.scene"})),
                    _ => ("editor.camera", json!({"fovYDegrees":"NaN"})),
                };
                Action::Protocol {
                    method: method.into(),
                    params,
                    raw: false,
                }
            }
            8 => {
                let end = [point[0] + 96., point[1] + 48.];
                self.pending.push_back(Action::Move { point: end });
                self.pending
                    .push_back(registry_action("edit.undo", Target::None));
                let cancel = self.index(2) == 0;
                self.pending
                    .push_back(Action::Release { point: end, cancel });
                Action::Press { point }
            }
            9 => {
                let panels = ["inspector", "hierarchy", "assets", "console", "settings"];
                let panel = panels[self.index(panels.len())];
                self.pending.push_back(registry_action(
                    "window.move.right",
                    Target::Panel(panel.into()),
                ));
                registry_action("window.float", Target::Panel(panel.into()))
            }
            10 => {
                let panels = ["inspector", "hierarchy", "assets", "console", "settings"];
                let panel = panels[self.index(panels.len())];
                self.pending.push_back(Action::Protocol {
                    method: "editor.layout.show".into(),
                    params: json!({"panel":panel}),
                    raw: false,
                });
                registry_action("window.close_panel", Target::Panel(panel.into()))
            }
            11 if !shell.floats.is_empty() => Action::FloatingInput {
                surface: shell.floats[self.index(shell.floats.len())].id.clone(),
                reverse: self.index(2) == 0,
            },
            12 => {
                let id = ["assets.show_all", "console.errors", "problems.warnings"][self.index(3)];
                for _ in 0..3 {
                    self.pending.push_back(registry_action(id, Target::None));
                }
                registry_action(id, Target::None)
            }
            _ => Action::Key { scancode: 41 },
        }
    }
}

fn registry_action(id: &str, target: Target) -> Action {
    Action::Command {
        id: id.into(),
        target,
    }
}

fn edge_target(index: usize) -> Target {
    match index {
        0 => Target::None,
        1 => Target::Entity(String::new()),
        2 => Target::Entity("@levels/deleted.scene#e_deleted1".into()),
        3 => Target::Asset("missing/deleted.asset".into()),
        4 => Target::Folder("missing/folder".into()),
        5 => Target::Field("MissingComponent.deleted#/999999999".into()),
        6 => Target::Component("MissingComponent".into()),
        7 => Target::Panel("missing-panel".into()),
        8 => Target::Node("deleted-node".into()),
        9 => Target::Doc("sc_deleted1".into()),
        10 => Target::Window("deleted-window".into()),
        _ => Target::Text("Transform.position=[1e308,-1e308,0]".into()),
    }
}

fn edge_params(index: usize) -> Value {
    match index {
        0 => Value::Null,
        1 => json!([]),
        2 => json!("NaN"),
        3 => json!(1e308),
        _ => {
            json!({"session":"deleted-session","entity":"@levels/deleted.scene#e_deleted1","value":"NaN","__monkeyInvalid":true})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_seed_including_zero_has_a_stable_distinct_stream() {
        for seed in [0, 1, 42, u64::MAX] {
            let mut first = Generator::new(seed);
            let mut replay = Generator::new(seed);
            let values: Vec<_> = (0..256).map(|_| first.random()).collect();
            let replayed: Vec<_> = (0..256).map(|_| replay.random()).collect();
            assert_eq!(values, replayed);
            assert!(values.windows(2).any(|pair| pair[0] != pair[1]));
        }
        assert_ne!(Generator::new(1).random(), Generator::new(2).random());
    }

    #[test]
    fn edge_targets_cover_every_registry_target_variant() {
        let values: Vec<_> = (0..TARGET_CASES)
            .map(|i| serde_json::to_string(&edge_target(i)).unwrap())
            .collect();
        assert!(values.iter().any(|v| v.contains("deleted-window")));
        assert!(values.iter().any(|v| v.contains("deleted.scene")));
        assert!(values.iter().any(|v| v.contains("1e308")));
    }
}
