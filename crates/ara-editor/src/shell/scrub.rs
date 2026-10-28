//! Label drags preview locally and commit one typed transaction per gesture.
use super::*;

pub(super) struct Scrub {
    pub(super) path: String,
    pub(super) axis: Option<usize>,
    pub(super) pointer: Option<String>,
    widget: String,
    start: f32,
    bases: Vec<(String, f64)>,
    delta: f64,
    step: f64,
    min: Option<f64>,
    max: Option<f64>,
    rgba: bool,
}
impl Scrub {
    fn target(&self) -> String {
        self.axis.map_or_else(
            || self.path.clone(),
            |axis| {
                format!(
                    "{}.{}",
                    self.path,
                    if self.rgba {
                        ["r", "g", "b", "a"][axis]
                    } else {
                        ["x", "y", "z", "w"][axis]
                    }
                )
            },
        )
    }
    /// Clamp the gesture for each entity independently, preserving multi-edit offsets.
    pub(super) fn values(&self) -> impl Iterator<Item = (&str, Value)> {
        self.bases.iter().map(|(entity, base)| {
            let value = (base + self.delta)
                .max(self.min.unwrap_or(f64::NEG_INFINITY))
                .min(self.max.unwrap_or(f64::INFINITY));
            // Integer values remain JSON integers; floats keep their full precision.
            let value = serde_json::from_str(&value.to_string()).unwrap_or_else(|_| json!(base));
            (entity.as_str(), value)
        })
    }
}
impl Shell {
    /// Protect a native numeric grip's authored field for the lifetime of its gesture.
    pub(crate) fn begin_native_scrub(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if !self.input.scrubbing() {
            return Ok(());
        }
        let path = self
            .input
            .captured()
            .and_then(|id| self.bindings.get(&id))
            .and_then(|binding| match binding {
                Binding::Field(path, axis) => Some(axis.map_or_else(
                    || path.clone(),
                    |axis| {
                        let rgba = self.fields.get(path).is_some_and(|field| {
                            matches!(
                                field.widget,
                                crate::inspector::Widget::Vector { rgba: true, .. }
                            )
                        });
                        format!(
                            "{path}.{}",
                            if rgba {
                                ["r", "g", "b", "a"][axis]
                            } else {
                                ["x", "y", "z", "w"][axis]
                            }
                        )
                    },
                )),
                Binding::Nested(target, ..) => Some(
                    target
                        .split_once('#')
                        .map_or(target.as_str(), |(path, _)| path)
                        .to_owned(),
                ),
                _ => None,
            });
        if let Some(path) = path {
            for entity in &editor.state.selection.clone() {
                editor.field_gesture(entity, &path, true)?;
            }
        }
        Ok(())
    }
    /// Begin a scalar or channel gesture using each selected entity's effective base.
    pub fn begin_scrub(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self
            .label_edge()
            .is_some_and(|edge| (self.pointer[0] - edge).abs() <= 4.)
        {
            return Ok(false);
        }
        let Some((target, axis, nested)) = self
            .ui
            .hit_test(self.pointer[0], self.pointer[1])
            .and_then(|i| {
                self.bindings
                    .get(&ara_ui::source::stable_id(&self.ui.nodes[i].element.id))
            })
            .and_then(|binding| match binding {
                Binding::Scrub(path, axis) => Some((path.clone(), *axis, None)),
                Binding::NestedScrub {
                    target,
                    widget,
                    step,
                    min,
                    max,
                } => Some((
                    target.clone(),
                    None,
                    Some((widget.clone(), *step, *min, *max)),
                )),
                _ => None,
            })
        else {
            return Ok(false);
        };
        let (path, pointer) = target
            .split_once('#')
            .map_or((target.as_str(), None), |(path, pointer)| {
                (path, Some(pointer.to_owned()))
            });
        let field = self.fields.get(path).ok_or("Scrub field disappeared")?;
        if field.read_only {
            return Ok(false);
        }
        let rgba = matches!(
            field.widget,
            crate::inspector::Widget::Vector { rgba: true, .. }
        );
        let (step, min, max) = if let Some((_, step, min, max)) = &nested {
            (*step, *min, *max)
        } else {
            match field.widget {
                crate::inspector::Widget::Number { step, min, max, .. } => (step, min, max),
                crate::inspector::Widget::Vector { color: true, .. } => (0.005, Some(0.), Some(1.)),
                _ => (0.01, None, None),
            }
        };
        let resolved = self.resolved(editor)?;
        let doc = editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("Scrub document")?;
        let mut bases = Vec::new();
        for entity in &editor.state.selection {
            let fields = crate::inspector::fields(
                editor.host.store().schema(),
                doc,
                &resolved,
                std::slice::from_ref(entity),
                &BTreeMap::new(),
            );
            if let Some(field) = fields.iter().find(|field| field.path == path)
                && let Some(value) = pointer.as_deref().map_or_else(
                    || axis.map_or_else(|| field.value.as_f64(), |axis| field.value[axis].as_f64()),
                    |pointer| field.value.pointer(pointer).and_then(Value::as_f64),
                )
            {
                bases.push((entity.clone(), value));
            }
        }
        if bases.is_empty() {
            return Ok(false);
        }
        let widget = nested.map(|(widget, ..)| widget).unwrap_or_else(|| {
            format!(
                "edit-{}{}",
                path.replace('.', "-"),
                axis.map(|axis| format!("-{axis}")).unwrap_or_default()
            )
        });
        let scrub = Scrub {
            path: path.to_owned(),
            axis,
            pointer,
            widget,
            start: self.pointer[0],
            bases,
            delta: 0.,
            step,
            min,
            max,
            rgba,
        };
        let target = scrub.target();
        for (entity, _) in &scrub.bases {
            editor.field_gesture(entity, &target, true)?;
        }
        self.scrub = Some(scrub);
        Ok(true)
    }
    /// Update only the local display and transient Scene preview, never authoring data.
    pub fn move_scrub(&mut self) -> ara_ui::Result<bool> {
        let Some(scrub) = &mut self.scrub else {
            return Ok(false);
        };
        let pixels = f64::from(self.pointer[0] - scrub.start);
        scrub.delta = if pixels.abs() < 2. {
            0.
        } else {
            pixels
                * scrub.step
                * if self.modifiers[0] {
                    0.1
                } else if self.modifiers[1] {
                    10.
                } else {
                    1.
                }
        };
        // Round integral schema steps before validating the typed edit.
        if scrub.step >= 1. && scrub.step.fract() == 0. {
            scrub.delta = scrub.delta.round();
        }
        // SceneSync keys transient previews by drafts; this marker is never authored.
        self.drafts
            .insert(format!("@scrub|{}", scrub.path), scrub.delta.to_string());
        if let Some((_, value)) = scrub.values().next()
            && let Some(i) = self.ui.find(&scrub.widget)
        {
            self.ui.set_text(i, &super::panels::display(&value))?;
        }
        Ok(true)
    }
    /// Release creates one undoable batch; Escape discards all preview state.
    pub fn finish_scrub(&mut self, editor: &mut Editor, cancel: bool) -> ara_ui::Result<bool> {
        let Some(scrub) = self.scrub.take() else {
            return Ok(false);
        };
        self.drafts.remove(&format!("@scrub|{}", scrub.path));
        let result = (|| -> ara_ui::Result<()> {
            if cancel || scrub.delta.abs() <= f64::EPSILON {
                return Ok(());
            }
            let target = scrub.pointer.as_ref().map_or_else(
                || scrub.target(),
                |pointer| format!("{}#{pointer}", scrub.path),
            );
            let mut calls = Vec::new();
            for (entity, value) in scrub.values() {
                if scrub
                    .bases
                    .iter()
                    .find(|(id, _)| id == entity)
                    .is_some_and(|(_, base)| value.as_f64() == Some(*base))
                {
                    continue;
                }
                calls.extend(self.field_commands(editor, &[entity.to_owned()], &target, value)?);
            }
            if !calls.is_empty() {
                editor.apply(calls, "Scrub inspector value", None)?;
            }
            Ok(())
        })();
        editor.end_field_gestures()?;
        result?;
        self.rebuild(editor)?;
        Ok(true)
    }
}
