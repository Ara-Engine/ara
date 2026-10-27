use super::*;
use ara_schema::{FieldDesc, Shape};
const PAGE: usize = 32;
fn token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
impl Shell {
    pub(super) fn play_runtime_markup(&mut self, editor: &Editor, out: &mut String) {
        let Some(runtime) = self.play_controls.runtime.take() else {
            out.push_str("<text id='runtime-inspect-hint' class='caption' text='Select a Hierarchy entity, then Inspect selected to read the controlled player’s runtime components.' />");
            return;
        };
        out.push_str(&format!("<text id='runtime-inspection-title' class='heading' text='Runtime · {} · {} · tick {}' /><text id='runtime-inspection-hint' class='caption' text='Values refresh on Inspect. Apply edits at a tick boundary; Keep saves selected changes to the scene.' />",escape(&runtime.entity),escape(&player_label(runtime.player,self.game_players.len())),runtime.tick));
        if !runtime.local {
            out.push_str("<text id='runtime-network-scope' class='caption' text='Network runtime values are read only. Keep saves the authority’s selected changes.' />");
        }
        if let Some(components) = runtime.components.as_object() {
            let db = editor.host.store().schema();
            for (component, value) in components {
                let Some(ty) = db.get(component) else {
                    continue;
                };
                out.push_str(&format!(
                    "<text id='runtime-component-{}' class='heading' text='{}' />",
                    ara_ui::source::stable_id(component),
                    escape(component)
                ));
                let value = db.canonical_value(&ValueType::Named(component.clone()), value);
                for desc in ty.fields().iter().filter(|desc| !desc.editor.hidden) {
                    let mut desc = desc.clone();
                    desc.editor.read_only |= !runtime.local || self.play_controls.pending.is_some();
                    let path = format!("{component}.{}", desc.name);
                    let field = value
                        .get(&desc.name)
                        .cloned()
                        .unwrap_or_else(|| db.field_default(&desc));
                    self.runtime_value(
                        editor, &runtime, &path, "", &path, &desc.ty, &field, &desc, 0, out,
                    );
                }
            }
        }
        self.play_controls.runtime = Some(runtime);
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "recursive reflected leaves retain their typed patch target and metadata"
    )]
    fn runtime_value(
        &mut self,
        editor: &Editor,
        runtime: &Runtime,
        path: &str,
        pointer: &str,
        label: &str,
        ty: &ValueType,
        value: &Value,
        desc: &FieldDesc,
        depth: usize,
        out: &mut String,
    ) {
        let key = format!("{path}#{pointer}");
        let id = format!("runtime-{}", ara_ui::source::stable_id(&key));
        if depth > 64 {
            out.push_str(&format!("<text id='{id}-limit' class='caption' text='{} · nested value exceeds the editor depth limit' />",escape(label)));
            return;
        }
        let target = FieldTarget {
            path: path.into(),
            pointer: pointer.into(),
            ty: ty.clone(),
            unit: desc.unit.map(|unit| unit.symbol().into()),
        };
        if matches!(ty, ValueType::Json) {
            let summary = if let Some(values) = value.as_array() {
                format!("{} values", values.len())
            } else if let Some(values) = value.as_object() {
                format!("{} fields", values.len())
            } else {
                panels::display(value).chars().take(512).collect()
            };
            out.push_str(&format!("<panel id='{id}' class='prop-row'><text id='{id}-label' class='prop-label' text='{}' /><text id='{id}-value' class='caption' style='white-space:normal' text='{}' description='Read-only runtime value' /></panel>",escape(label),escape(&summary)));
            return;
        }
        if let ValueType::Option(inner) = ty {
            let checked = !value.is_null();
            if desc.editor.read_only {
                if checked {
                    self.runtime_value(
                        editor,
                        runtime,
                        path,
                        pointer,
                        label,
                        inner,
                        value,
                        desc,
                        depth + 1,
                        out,
                    );
                } else {
                    out.push_str(&format!(
                        "<text id='{id}-none' class='caption' text='{} · None' />",
                        escape(label)
                    ));
                }
                return;
            }
            self.bindings.insert(
                ara_ui::source::stable_id(&format!("{id}-enabled")),
                Binding::Play(Action::Set(
                    target,
                    if checked {
                        Value::Null
                    } else {
                        editor.host.store().schema().zero(inner)
                    },
                )),
            );
            out.push_str(&format!("<toggle id='{id}-enabled' checked='{checked}' label='Set {}' description='Enable or clear this optional runtime value' action='Dispatch' />",escape(label)));
            if checked {
                self.runtime_value(
                    editor,
                    runtime,
                    path,
                    pointer,
                    label,
                    inner,
                    value,
                    desc,
                    depth + 1,
                    out,
                );
            }
            return;
        }
        if let Some(count) = ty.vector_len() {
            for axis in 0..count {
                let label = format!("{label}.{}", ["x", "y", "z", "w"][axis]);
                let value = value.get(axis).cloned().unwrap_or_else(|| json!(0.));
                self.runtime_value(
                    editor,
                    runtime,
                    path,
                    &format!("{pointer}/{axis}"),
                    &label,
                    &ValueType::F32,
                    &value,
                    desc,
                    depth + 1,
                    out,
                );
            }
            return;
        }
        if let ValueType::Named(name) = ty
            && let Some(named) = editor.host.store().schema().get(name)
        {
            match &named.shape {
                Shape::Enum { variants } => {
                    if desc.editor.read_only {
                        self.runtime_read_only(&id, label, value, out);
                        return;
                    }
                    out.push_str(&format!("<panel id='{id}-row' class='prop-row'><text id='{id}-label' class='prop-label' text='{}' />",escape(label)));
                    self.choice_button(
                        &id,
                        label,
                        choices::Select {
                            destination: choices::Destination::PlayField(target),
                            options: variants
                                .iter()
                                .map(|variant| {
                                    (
                                        json!(variant.name),
                                        super::super::inspector_panel::sentence(&variant.name),
                                    )
                                })
                                .collect(),
                            current: value.clone(),
                        },
                        out,
                    );
                    out.push_str("</panel>");
                    return;
                }
                Shape::Struct { fields, .. } => {
                    if self.runtime_expand(&id, &key, label, fields.len(), out) {
                        for field in fields.iter().filter(|field| !field.editor.hidden) {
                            let mut field = field.clone();
                            field.editor.read_only |= desc.editor.read_only;
                            let child = value.get(&field.name).cloned().unwrap_or_else(|| {
                                editor.host.store().schema().field_default(&field)
                            });
                            self.runtime_value(
                                editor,
                                runtime,
                                path,
                                &format!("{pointer}/{}", token(&field.name)),
                                &format!("{label}.{}", field.name),
                                &field.ty,
                                &child,
                                &field,
                                depth + 1,
                                out,
                            );
                        }
                    }
                    return;
                }
            }
        }
        if let ValueType::List(inner) | ValueType::Keyed(inner) = ty {
            let count = value.as_array().map_or(0, Vec::len);
            if self.runtime_expand(&id, &key, label, count, out) {
                let page = self.runtime_page(&id, &key, count, out);
                for (index, child) in value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .skip(page * PAGE)
                    .take(PAGE)
                {
                    self.runtime_value(
                        editor,
                        runtime,
                        path,
                        &format!("{pointer}/{index}"),
                        &format!("{label}[{index}]"),
                        inner,
                        child,
                        desc,
                        depth + 1,
                        out,
                    );
                }
            }
            return;
        }
        if let ValueType::Map(inner) = ty {
            let count = value.as_object().map_or(0, serde_json::Map::len);
            if self.runtime_expand(&id, &key, label, count, out) {
                let page = self.runtime_page(&id, &key, count, out);
                for (name, child) in value
                    .as_object()
                    .into_iter()
                    .flat_map(|map| map.iter())
                    .skip(page * PAGE)
                    .take(PAGE)
                {
                    self.runtime_value(
                        editor,
                        runtime,
                        path,
                        &format!("{pointer}/{}", token(name)),
                        &format!("{label}[{name}]"),
                        inner,
                        child,
                        desc,
                        depth + 1,
                        out,
                    );
                }
            }
            return;
        }
        if desc.editor.read_only {
            self.runtime_read_only(&id, label, value, out);
            return;
        }
        self.bindings.insert(
            ara_ui::source::stable_id(&id),
            Binding::Play(Action::Field(target.clone())),
        );
        let shown = runtime
            .drafts
            .get(&target.key())
            .map(|draft| &draft.input)
            .unwrap_or(value);
        out.push_str(&format!("<panel id='{id}-row' class='prop-row'><text id='{id}-label' class='prop-label' text='{}' description='{}' />",escape(label),escape(&desc.doc)));
        if *ty == ValueType::Bool {
            out.push_str(&format!(
                "<toggle id='{id}' checked='{}' label='{}' action='Dispatch' />",
                shown.as_bool().unwrap_or(false),
                escape(label)
            ));
        } else {
            let bounds = desc
                .range
                .map(|range| {
                    format!(
                        "{}{}",
                        range
                            .min
                            .map(|min| format!(" min='{min}'"))
                            .unwrap_or_default(),
                        range
                            .max
                            .map(|max| format!(" max='{max}'"))
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_default();
            out.push_str(&format!("<field id='{id}' class='{}' style='flex-grow:1;min-width:60px' text='{}' label='{}' description='{} · Apply runtime edits, or press Enter' {} step='0.01' action='Dispatch' />",if ty.is_number(){"number"}else{""},escape(&panels::display(shown)),escape(label),escape(&desc.doc),bounds));
        }
        out.push_str("</panel>");
    }
    fn runtime_read_only(&self, id: &str, label: &str, value: &Value, out: &mut String) {
        out.push_str(&format!("<panel id='{id}-readonly' class='prop-row'><text id='{id}-label' class='prop-label' text='{}' /><text id='{id}-value' class='caption' text='{}' description='Read-only runtime value' /></panel>",escape(label),escape(&panels::display(value).chars().take(512).collect::<String>())));
    }
    fn runtime_expand(
        &mut self,
        id: &str,
        key: &str,
        label: &str,
        count: usize,
        out: &mut String,
    ) -> bool {
        let expanded = self.play_controls.expanded.contains(key);
        self.bindings.insert(
            ara_ui::source::stable_id(&format!("{id}-expand")),
            Binding::Play(Action::Expand(key.into())),
        );
        out.push_str(&format!("<button id='{id}-expand' class='ghost' label='{} {}' action='Dispatch'><text id='{id}-expand-label' text='{} {} · {count}' /></button>",if expanded{"Collapse"}else{"Expand"},escape(label),if expanded{"▾"}else{"▸"},escape(label)));
        expanded
    }
    fn runtime_page(&mut self, id: &str, key: &str, count: usize, out: &mut String) -> usize {
        let last = count.saturating_sub(1) / PAGE;
        let page = self.play_controls.pages.entry(key.into()).or_default();
        *page = (*page).min(last);
        let page = *page;
        if last > 0 {
            for (name, delta, disabled) in [("previous", -1, page == 0), ("next", 1, page == last)]
            {
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("{id}-{name}")),
                    Binding::Play(Action::Page(key.into(), delta)),
                );
                out.push_str(&format!("<button id='{id}-{name}' class='ghost' label='{name} runtime items' disabled='{disabled}' action='Dispatch'><text id='{id}-{name}-label' text='{name}' /></button>"));
            }
            out.push_str(&format!(
                "<text id='{id}-page' class='caption' text='Items {}–{} of {count}' />",
                page * PAGE + 1,
                ((page + 1) * PAGE).min(count)
            ));
        }
        page
    }
}
