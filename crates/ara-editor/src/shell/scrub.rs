//! Numeric label drags stage values locally and commit exactly one content batch.
use super::*;
pub(super) struct Scrub {
    path: String,
    axis: usize,
    start: f32,
    bases: Vec<(String, f64)>,
    delta: f64,
    step: f64,
    color: bool,
}
impl Shell {
    /// Begin a numeric label gesture using per-entity bases, including mixed selections.
    pub fn begin_scrub(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some((path, axis)) = self
            .ui
            .hit_test(self.pointer[0], self.pointer[1])
            .and_then(|i| {
                self.bindings
                    .get(&ara_ui::source::stable_id(&self.ui.nodes[i].element.id))
            })
            .and_then(|b| {
                if let Binding::Scrub(p, a) = b {
                    Some((p.clone(), *a))
                } else {
                    None
                }
            })
        else {
            return Ok(false);
        };
        let field = self.fields.get(&path).ok_or("scrub field disappeared")?;
        let color = matches!(
            field.widget,
            crate::inspector::Widget::Vector { rgba: true, .. }
        );
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?;
        let doc = editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("scrub document")?;
        let mut bases = Vec::new();
        for entity in &editor.state.selection {
            let fields = crate::inspector::fields(
                editor.host.store().schema(),
                doc,
                &resolved,
                std::slice::from_ref(entity),
                &BTreeMap::new(),
            );
            if let Some(f) = fields.iter().find(|f| f.path == path) {
                bases.push((entity.clone(), f.value[axis].as_f64().unwrap_or(0.)));
            }
        }
        let gesture_path = format!(
            "{path}.{}",
            if color {
                ["r", "g", "b", "a"][axis]
            } else {
                ["x", "y", "z", "w"][axis]
            }
        );
        for (entity, _) in &bases {
            editor.field_gesture(entity, &gesture_path, true)?;
        }
        self.scrub = Some(Scrub {
            path,
            axis,
            start: self.pointer[0],
            bases,
            delta: 0.,
            step: if color { 0.005 } else { 0.01 },
            color,
        });
        Ok(true)
    }
    /// Update the staged numeric display without mutating project documents.
    pub fn move_scrub(&mut self) -> ara_ui::Result<bool> {
        let Some(s) = &mut self.scrub else {
            return Ok(false);
        };
        s.delta = f64::from(self.pointer[0] - s.start)
            * s.step
            * if self.modifiers[0] {
                0.1
            } else if self.modifiers[1] {
                10.
            } else {
                1.
            };
        let value = s.bases.first().map_or(0., |(_, v)| *v) + s.delta;
        if let Some(i) = self
            .ui
            .find(&format!("edit-{}-{}", s.path.replace('.', "-"), s.axis))
        {
            self.ui.set_text(i, &format!("{value:.3}"))?;
        }
        Ok(true)
    }
    /// Release or cancel the local preview. A release creates one undoable batch.
    pub fn finish_scrub(&mut self, editor: &mut Editor, cancel: bool) -> ara_ui::Result<bool> {
        let Some(s) = self.scrub.take() else {
            return Ok(false);
        };
        if !cancel && s.delta.abs() > f64::EPSILON {
            let path = format!(
                "{}.{}",
                s.path,
                if s.color {
                    ["r", "g", "b", "a"][s.axis]
                } else {
                    ["x", "y", "z", "w"][s.axis]
                }
            );
            let commands = s
                .bases
                .into_iter()
                .flat_map(|(entity, value)| {
                    crate::inspector::edit_commands(
                        &[entity],
                        &path,
                        &json!(if s.color {
                            (value + s.delta).clamp(0., 1.)
                        } else {
                            value + s.delta
                        }),
                    )
                })
                .collect();
            editor.apply(commands, "Scrub inspector value", None)?;
        }
        editor.end_field_gestures()?;
        self.rebuild(editor)?;
        Ok(true)
    }
}
