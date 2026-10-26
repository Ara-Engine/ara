//! Scalar, reference and colour controls inside schema-driven compound fields.
use super::*;

impl Shell {
    /// The widget for one scalar, vector, reference or enum element.
    pub(super) fn leaf(
        &mut self,
        editor: &Editor,
        field: &Field,
        row: &Row<'_>,
        rid: &str,
        out: &mut String,
    ) {
        let db = editor.host.store().schema();
        let selection = &editor.state.selection;
        let (desc, pointer) = (row.desc, row.pointer);
        let nullable = matches!(row.ty, ValueType::Option(_));
        let ty = inner(row.ty);
        let fallback = desc
            .map(|desc| crate::inspector::override_default(db, desc))
            .unwrap_or_else(|| db.zero(ty));
        let value = if nullable && row.value.is_null() {
            &fallback
        } else {
            row.value
        };
        let target = &format!("{}#{pointer}", field.path);
        let set = |to: Value| set_commands(selection, field, pointer, to);
        let id = format!("nest-edit-{rid}");
        let text_box = |shell: &mut Self,
                        out: &mut String,
                        id: &str,
                        target: String,
                        leaf: Leaf,
                        text: String,
                        numeric: bool| {
            shell.bindings.insert(
                ara_ui::source::stable_id(id),
                Binding::Nested(
                    target,
                    leaf,
                    nullable,
                    desc.and_then(|desc| desc.unit)
                        .map(|unit| unit.symbol().to_owned()),
                ),
            );
            out.push_str(&format!("<field id='{id}' class='{}' style='flex-grow:1;flex-basis:0px' text='{}' label='{}' description='{}' action='Dispatch' />",if numeric{"number"}else{""},escape(&text),escape(&sentence(pointer.rsplit('/').next().unwrap_or(pointer))),escape(desc.map_or("",|d|d.doc.as_str()))));
        };
        if ty.is_number() {
            let range = desc.and_then(|d| d.range);
            if let Some((min, max)) = range.and_then(|r| Some((r.min?, r.max?))) {
                let slider = format!("nest-slider-{rid}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&slider),
                    Binding::Nested(
                        target.to_owned(),
                        Leaf::Number,
                        false,
                        desc.and_then(|desc| desc.unit)
                            .map(|unit| unit.symbol().to_owned()),
                    ),
                );
                out.push_str(&format!("<slider id='{slider}' style='flex-grow:1;flex-basis:0px' min='{min}' max='{max}' step='{}' value='{}' action='Dispatch' />",desc.and_then(|d|d.editor.step).unwrap_or(0.01),value.as_f64().unwrap_or(min).clamp(min,max)));
            }
            text_box(
                self,
                out,
                &id,
                target.to_owned(),
                Leaf::Number,
                display(value),
                true,
            );
            if let Some(unit) = desc.and_then(|d| d.unit) {
                out.push_str(&format!(
                    "<text id='unit-{rid}' class='caption' text='{}' />",
                    escape(unit.symbol())
                ));
            }
            return;
        }
        if let Some(count) = ty.vector_len() {
            if *ty == ValueType::Color || desc.is_some_and(|d| d.editor.color) {
                let swatch = format!("nest-swatch-{rid}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&swatch),
                    Binding::ColorPicker(target.to_owned()),
                );
                out.push_str(&format!("<button id='{swatch}' class='swatch inspector-swatch' label='Pick colour' description='Open the colour picker' action='Dispatch' />"));
                self.nested_swatches.push((swatch, value.clone()));
                text_box(
                    self,
                    out,
                    &id,
                    target.to_owned(),
                    Leaf::Hex,
                    linear_hex(value),
                    false,
                );
                return;
            }
            for axis in 0..count {
                let cell = format!("{id}-{axis}");
                if !field.read_only
                    && !row.value.is_null()
                    && !desc.is_some_and(|desc| desc.editor.read_only)
                {
                    self.bindings.insert(
                        ara_ui::source::stable_id(&format!("{cell}-axis")),
                        Binding::NestedScrub {
                            target: format!("{target}/{axis}"),
                            widget: cell.clone(),
                            step: desc.and_then(|desc| desc.editor.step).unwrap_or(0.01),
                            min: desc.and_then(|desc| desc.range).and_then(|range| range.min),
                            max: desc.and_then(|desc| desc.range).and_then(|range| range.max),
                        },
                    );
                }
                out.push_str(&format!("<panel id='{cell}-cell' class='vector-cell'><button id='{cell}-axis' class='axis axis-{}' cursor='ew-resize' label='Scrub {}' description='Drag to scrub; Shift: fine, Ctrl: coarse; Esc cancels.' action='Dispatch'><text id='{cell}-axis-label' text='{}' /></button>",["x","y","z","w"][axis],["X","Y","Z","W"][axis],["X","Y","Z","W"][axis]));
                text_box(
                    self,
                    out,
                    &cell,
                    format!("{target}/{axis}"),
                    Leaf::Number,
                    display(&value[axis]),
                    true,
                );
                out.push_str("</panel>");
            }
            return;
        }
        match ty {
            ValueType::Bool => {
                let on = value.as_bool().unwrap_or(false);
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Content(
                        set(json!(!on)),
                        format!("Set {}", sentence(&field.label).to_lowercase()),
                    ),
                );
                out.push_str(&format!(
                    "<toggle id='{id}' checked='{on}' label='{}' action='Dispatch' />",
                    escape(&sentence(pointer.rsplit('/').next().unwrap_or(pointer)))
                ));
            }
            ValueType::Asset(kind) => {
                let reference = value.as_str().unwrap_or("");
                let source = reference
                    .trim_start_matches('@')
                    .split('#')
                    .next()
                    .unwrap_or("");
                // Without its extension, like Assets; the tooltip has the reference.
                let name = Some(names::stem(names::file_name(source)))
                    .filter(|s| !s.is_empty())
                    .unwrap_or("None");
                let button = format!("asset-field-{rid}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&button),
                    Binding::AssetPicker(target.to_owned(), kind.clone()),
                );
                let preview = match self.thumbnail_slots.get(source) {
                    Some(slot) => format!(
                        "<image id='{button}-thumbnail' src='{}' style='width:16px;height:16px' />",
                        crate::asset_tools::thumbnails::image_name(*slot)
                    ),
                    None => {
                        if !source.is_empty()
                            && matches!(
                                source.rsplit('.').next(),
                                Some("material" | "graph" | "png" | "jpg" | "jpeg")
                            )
                        {
                            self.thumbnails.request(source);
                        }
                        icons::icon(&button, "package")
                    }
                };
                out.push_str(&format!("<button id='{button}' style='flex-grow:1;flex-basis:0px;min-width:0px;overflow:hidden;justify-content:start;background:var(--background);border-color:var(--input)' label='{} asset' description='{} · drop an asset here, or pick one' action='Dispatch'>{preview}<text id='asset-name-{rid}' class='{}' style='white-space:nowrap;overflow:hidden;min-width:0px;flex-shrink:1' text='{}' /></button>",escape(kind.as_deref().unwrap_or("Any")),escape(reference),if reference.is_empty(){"muted"}else{"fg"},escape(name)));
                if !reference.is_empty() {
                    let clear = format!("clear-{rid}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&clear),
                        Binding::Content(
                            set(Value::Null),
                            format!("Clear {}", sentence(&field.label).to_lowercase()),
                        ),
                    );
                    out.push_str(&format!("<button id='{clear}' class='ghost icon-button' label='Clear' description='Clear the reference' action='Dispatch'>{}</button>",icons::icon_in(&clear,"x","icon-sm")));
                }
            }
            ValueType::Named(name) => {
                let Some(Shape::Enum { variants }) = db.get(name).map(|t| &t.shape) else {
                    return;
                };
                let options: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::EnumMenu(target.to_owned(), options),
                );
                out.push_str(&format!("<button id='{id}' style='flex-grow:1;flex-basis:0px;justify-content:start' label='{}' action='Dispatch'><text id='{id}-value' class='fill' text='{}' />{}</button>",escape(name),escape(&sentence(value.as_str().unwrap_or(""))),icons::icon_in(&id,"chevron-down","icon-sm")));
            }
            ValueType::Json => {
                let text = if value.is_null() {
                    String::new()
                } else {
                    value.to_string()
                };
                text_box(self, out, &id, target.to_owned(), Leaf::Json, text, false);
            }
            // Strings and entity addresses.
            _ => {
                let nullable = nullable || *ty == ValueType::Entity;
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Nested(target.to_owned(), Leaf::Text, nullable, None),
                );
                out.push_str(&format!("<field id='{id}' style='flex-grow:1;flex-basis:0px' text='{}' label='{}' description='{}' action='Dispatch' />",escape(value.as_str().unwrap_or("")),escape(&sentence(pointer.rsplit('/').next().unwrap_or(pointer))),escape(desc.map_or("",|d|d.doc.as_str()))));
                if *ty == ValueType::Entity {
                    let id = format!("pick-{rid}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&id),
                        Binding::PickField(target.to_owned()),
                    );
                    out.push_str(&format!("<button id='{id}' class='ghost icon-button' label='Pick from scene' description='Click an entity in the Scene or Hierarchy' action='Dispatch'>{}</button>",icons::icon_in(&id,"crosshair","icon-sm")));
                }
            }
        }
    }

    /// Paints the swatches of nested colour elements.
    pub(in crate::shell) fn paint_nested_swatches(&mut self) {
        for (id, value) in &self.nested_swatches {
            let Some(i) = self.shown_node(id) else {
                continue;
            };
            let r = self.ui.nodes[i].rect;
            let clip = self.ui.nodes[i].clip.intersection(r);
            if clip.w <= 0. || clip.h <= 0. {
                continue;
            }
            let channel = |n: usize| value[n].as_f64().unwrap_or(1.).clamp(0., 1.);
            let display = &mut self.ui.display.commands;
            display.push(ara_ui::paint::Draw::PushClip { rect: clip });
            display.push(ara_ui::paint::Draw::RoundedRect {
                node: ara_ui::source::stable_id(id),
                rect: Box2 {
                    x: r.x + 2.,
                    y: r.y + 2.,
                    w: (r.w - 4.).max(0.),
                    h: (r.h - 4.).max(0.),
                },
                radius: 3.,
                color: ara_ui::style::Color(
                    (channel(0).powf(1. / 2.2) * 255.) as u8,
                    (channel(1).powf(1. / 2.2) * 255.) as u8,
                    (channel(2).powf(1. / 2.2) * 255.) as u8,
                    (value
                        .get(3)
                        .and_then(Value::as_f64)
                        .unwrap_or(1.)
                        .clamp(0., 1.)
                        * 255.) as u8,
                ),
            });
            display.push(ara_ui::paint::Draw::PopClip);
        }
    }
}
