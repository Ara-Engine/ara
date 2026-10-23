//! Ordered emitter/module stacks. Graph topology is never shown as a VFX UI.
use super::*;
impl Shell {
    pub(super) fn vfx_panel(&mut self, editor: &Editor, out: &mut String) -> ara_ui::Result<()> {
        let resource = self.resource_for(editor, "vfx");
        self.resource_toolbar(editor, out, "vfx", resource.as_deref());
        let Some(doc) = resource else {
            self.empty(
                out,
                "vfx-empty",
                "sparkles",
                "No effect open",
                "Open a VFX graph from Assets, or create one.",
                &[("vfx-empty-new", "New effect", "assets.create.vfx")],
            );
            return Ok(());
        };
        let graph = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(&doc)
                .ok_or("effect missing")?,
        )
        .map_err(|d| d.to_string())?;
        let effect = ara_vfx::Effect::from_graph(&graph).map_err(|d| d.to_string())?;
        self.vfx_visible = true;
        self.vfx_stream
            .show(effect.clone(), editor.state.preview_playing);
        out.push_str("<panel id='vfx-content' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px'><scroll id='vfx-stack-list' style='width:300px;flex-basis:300px;flex-shrink:0;padding:6px;gap:0px;background:var(--panel-header)'>");
        for emitter in &effect.emitters {
            out.push_str(&format!("<panel id='emitter-{}' class='row' style='padding-left:4px;height:26px;flex-shrink:0'>{}<text id='emitter-title-{}' class='heading' text='{}' /><panel id='emitter-fill-{}' class='fill' /><text id='emitter-capacity-{}' class='caption' text='{} max' /></panel>",escape(&emitter.name),icons::icon(&emitter.name,"sparkles"),escape(&emitter.name),escape(&emitter.name),escape(&emitter.name),escape(&emitter.name),emitter.capacity));
            for stage in ara_vfx::Stacks::STAGES {
                let stage_name = serde_json::to_value(stage)?
                    .as_str()
                    .ok_or("stage name")?
                    .to_owned();
                let modules = match stage {
                    ara_vfx::Stage::Spawn => &emitter.stacks.spawn,
                    ara_vfx::Stage::Initialize => &emitter.stacks.initialize,
                    ara_vfx::Stage::Update => &emitter.stacks.update,
                    ara_vfx::Stage::Render => &emitter.stacks.render,
                };
                let rail = match stage {
                    ara_vfx::Stage::Spawn => "success-foreground",
                    ara_vfx::Stage::Initialize => "chart-2",
                    ara_vfx::Stage::Update => "warning-foreground",
                    ara_vfx::Stage::Render => "chart-4",
                };
                let module = default_module(stage, editor.host.store().revision());
                let add = format!("module-add-{}-{stage_name}", emitter.name);
                self.bindings.insert(ara_ui::source::stable_id(&add), Binding::ResourceContent(doc.clone(),vec![json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter.name,"stage":stage_name,"name":module.name,"operation":"insert","index":modules.len(),"module":module}})],"Add VFX module".into()));
                out.push_str(&format!("<panel id='stage-{}-{stage_name}' class='stage-header'><panel id='stage-rail-{}-{stage_name}' class='stage-rail' style='background:var(--{rail})' /><text id='stage-label-{}-{stage_name}' class='caption' text='{}' /><panel id='stage-fill-{}-{stage_name}' class='fill' /><button id='{add}' class='ghost icon-button' label='Add module' description='Add a {} module' action='Dispatch'>{}</button></panel>",emitter.name,emitter.name,emitter.name,stage_name.to_uppercase(),emitter.name,stage_name,icons::icon(&add,"plus")));
                for module in modules.iter() {
                    let item = format!("{}|{stage_name}|{}", emitter.name, module.name);
                    let id = format!("module-{}-{stage_name}-{}", emitter.name, module.name);
                    let value = ara_json::to_value(&module.module)?;
                    let kind = value["op"].as_str().unwrap_or("module");
                    let selected = editor.state.resource_item.as_deref() == Some(&item);
                    let summary = value
                        .as_object()
                        .map(|m| {
                            m.iter()
                                .filter(|(k, _)| *k != "op")
                                .take(2)
                                .map(|(k, v)| {
                                    format!(
                                        "{} {}",
                                        super::inspector_panel::sentence(k).to_lowercase(),
                                        short(v)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(" · ")
                        })
                        .unwrap_or_default();
                    self.bindings.insert(
                        ara_ui::source::stable_id(&id),
                        Binding::Protocol("editor.workspace", json!({"resource":doc,"item":item})),
                    );
                    let title = if module.name == kind {
                        super::inspector_panel::sentence(kind)
                    } else {
                        format!(
                            "{} · {}",
                            module.name,
                            super::inspector_panel::sentence(kind).to_lowercase()
                        )
                    };
                    out.push_str(&format!("<panel id='{id}-card' class='module-row{}'><panel id='{id}-rail' class='stage-rail' style='height:14px;background:var(--{rail})' /><button id='{id}' class='module-title' label='{}' description='{}' action='Dispatch'><text id='{id}-name' class='{}' text='{}' /><panel id='{id}-gap' class='fill' /><text id='{id}-summary' class='{}' style='font-size:11px' text='{}' /></button>",if selected{" module-selected"}else{""},escape(&module.name),escape(ara_vfx::library::MODULES.iter().find(|k|k.name==kind).map(|k|k.doc).unwrap_or("Typed VFX module")),if selected{"on-selection"}else{"fg"},escape(&title),if selected{"on-selection"}else{"caption"},escape(&summary)));
                    if selected {
                        for (command, icon, label) in [
                            ("vfx.module.up", "chevron-up", "Move up · Alt+Up"),
                            ("vfx.module.down", "chevron-down", "Move down · Alt+Down"),
                            ("vfx.module.duplicate", "copy", "Duplicate · Ctrl+D"),
                            ("vfx.module.delete", "trash", "Delete · Del"),
                        ] {
                            let button =
                                format!("{id}-{}", command.rsplit('.').next().unwrap_or(""));
                            self.bind_command(
                                &button,
                                command,
                                commands::Target::Text(item.clone()),
                            );
                            out.push_str(&format!("<button id='{button}' class='ghost icon-button' style='width:20px;height:20px;padding:2px' label='{label}' description='{label}' action='Dispatch'>{}</button>",icons::icon_in(&button,icon,"on-selection")));
                        }
                    }
                    out.push_str("</panel>");
                }
            }
        }
        out.push_str("</scroll><panel id='vfx-preview-area' style='flex-grow:1;flex-basis:0px;padding:8px;gap:8px;background:var(--background)'><panel id='vfx-preview-tools' class='row'>");
        self.bindings.insert(
            ara_ui::source::stable_id("vfx-preview-play"),
            Binding::VfxPlayback(!editor.state.preview_playing),
        );
        out.push_str(&format!("<button id='vfx-preview-play' class='ghost' label='{}' description='Run or pause the fixed-tick preview' action='Dispatch'>{}<text id='vfx-preview-play-label' text='{}' /></button>",if editor.state.preview_playing{"Pause"}else{"Play"},icons::icon("vfx-preview-play",if editor.state.preview_playing{"pause"}else{"play"}),if editor.state.preview_playing{"Pause"}else{"Play"}));
        out.push_str(&format!("<text id='vfx-preview-title' class='heading' text='{}' /><panel id='vfx-preview-fill' class='fill' /><text id='vfx-preview-rate' class='chip-success' text='fixed {} Hz ✓' /></panel>",escape(&effect.name),effect.hz));
        if self.vfx_ready {
            let size = self
                .ui
                .find("vfx-preview-area")
                .map(|i| {
                    let r = self.ui.nodes[i].rect;
                    (r.w - 24.).min(r.h - 100.)
                })
                .unwrap_or(320.)
                .max(96.);
            out.push_str(&format!("<panel id='vfx-live-frame' style='flex-grow:1;flex-basis:0px;align-items:center;justify-content:center;overflow:hidden'><image id='vfx-live-image' src='vfx-live-preview' style='width:{size}px;height:{size}px' label='Live GPU particle preview' /></panel>"));
        } else {
            out.push_str("<panel id='vfx-preview-loading' style='flex-grow:1;align-items:center;justify-content:center'><text id='vfx-preview-loading-title' class='muted' text='Preparing effect preview...' /></panel>");
        }
        out.push_str("<text id='vfx-preview-detail' class='caption' text='Edits compile in the background; the last good preview stays visible.' style='white-space:normal' />");
        if let Some(error) = &self.vfx_error {
            out.push_str(&format!("<text id='vfx-preview-error' class='destructive' text='{}' style='white-space:normal' />",escape(error)));
        }
        out.push_str("</panel></panel>");
        Ok(())
    }
    pub(super) fn vfx_inspector(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let Some(doc) = self.resource_for(editor, "vfx") else {
            return Ok(());
        };
        let graph = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(&doc)
                .ok_or("effect missing")?,
        )
        .map_err(|d| d.to_string())?;
        let effect = ara_vfx::Effect::from_graph(&graph).map_err(|d| d.to_string())?;
        out.push_str("<scroll id='vfx-inspector' class='body'><text id='vfx-inspector-title' class='heading' text='Module parameters' />");
        if let Some(item) = editor.state.resource_item.as_deref() {
            let parts = item.split('|').collect::<Vec<_>>();
            if let [emitter, stage, module] = parts.as_slice() {
                let emitter = effect
                    .emitters
                    .iter()
                    .find(|e| e.name == *emitter)
                    .ok_or("emitter missing")?;
                let stage_value: ara_vfx::Stage = serde_json::from_value(json!(stage))?;
                let modules = match stage_value {
                    ara_vfx::Stage::Spawn => &emitter.stacks.spawn,
                    ara_vfx::Stage::Initialize => &emitter.stacks.initialize,
                    ara_vfx::Stage::Update => &emitter.stacks.update,
                    ara_vfx::Stage::Render => &emitter.stacks.render,
                };
                if let Some(module) = modules.iter().find(|m| m.name == *module) {
                    let value = ara_json::to_value(&module.module)?;
                    let kind = value["op"].as_str().unwrap_or("");
                    out.push_str(&format!("<text id='vfx-module-name' text='{}' /><text id='vfx-module-doc' class='muted' text='{}' style='white-space:normal' />",escape(&module.name),escape(ara_vfx::library::MODULES.iter().find(|k|k.name==kind).map(|k|k.doc).unwrap_or("Typed VFX module"))));
                    if let Some(fields) = value.as_object() {
                        for (key, v) in fields {
                            if key == "op" {
                                continue;
                            }
                            let id = format!("vfx-param-{key}");
                            out.push_str(&format!("<panel id='{id}-row' class='field-row'><text id='{id}-label' class='field-label' text='{}' />",escape(key)));
                            self.bindings.insert(
                                ara_ui::source::stable_id(&id),
                                Binding::VfxField(doc.clone(), item.into(), key.clone()),
                            );
                            if let Some(v) = v.as_bool() {
                                out.push_str(&format!("<toggle id='{id}' checked='{v}' text='Enabled' action='Dispatch' />"));
                            } else {
                                out.push_str(&format!("<field id='{id}' class='number' text='{}' label='{}' action='Dispatch' />",escape(&panels::display(v)),escape(key)));
                            }
                            out.push_str("</panel>");
                        }
                    }
                }
            }
        } else {
            out.push_str("<text id='vfx-module-empty' class='muted' text='Select a module to edit its parameters.' style='white-space:normal' />");
        }
        out.push_str("</scroll>");
        Ok(())
    }
    pub(super) fn set_vfx_field(
        &mut self,
        editor: &mut Editor,
        doc: &str,
        item: &str,
        key: &str,
        value: Value,
    ) -> ara_ui::Result<()> {
        let parts = item.split('|').collect::<Vec<_>>();
        let [emitter, stage, name] = parts.as_slice() else {
            return Err("invalid module address".into());
        };
        let graph = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(doc)
                .ok_or("effect missing")?,
        )
        .map_err(|d| d.to_string())?;
        let effect = ara_vfx::Effect::from_graph(&graph).map_err(|d| d.to_string())?;
        let stage_value: ara_vfx::Stage = serde_json::from_value(json!(stage))?;
        let emitter = effect
            .emitters
            .iter()
            .find(|e| e.name == *emitter)
            .ok_or("emitter missing")?;
        let modules = match stage_value {
            ara_vfx::Stage::Spawn => &emitter.stacks.spawn,
            ara_vfx::Stage::Initialize => &emitter.stacks.initialize,
            ara_vfx::Stage::Update => &emitter.stacks.update,
            ara_vfx::Stage::Render => &emitter.stacks.render,
        };
        let module = modules
            .iter()
            .find(|m| m.name == *name)
            .ok_or("module missing")?;
        let mut settings = ara_json::to_value(&module.module)?;
        settings[key] = value;
        editor.apply_to(doc,vec![json!({"cmd":"editor.vfx.module.set","args":{"emitter":emitter.name,"stage":stage,"name":name,"module":settings}})],"Edit VFX module parameter")?;
        Ok(())
    }
}
/// A compact value for inline module summaries.
fn short(v: &Value) -> String {
    match v {
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.);
            if f.fract() == 0. {
                format!("{f:.0}")
            } else {
                format!("{f:.2}").trim_end_matches('0').to_owned()
            }
        }
        Value::Array(a) => a.iter().map(short).collect::<Vec<_>>().join(","),
        Value::String(s) => s.clone(),
        Value::Bool(b) => {
            if *b {
                "on".into()
            } else {
                "off".into()
            }
        }
        _ => "…".into(),
    }
}
impl Shell {
    /// Reorder, duplicate or delete a module of the open effect (target `emitter|stage|name`).
    pub(super) fn edit_vfx_module(
        &mut self,
        editor: &mut Editor,
        item: &str,
        operation: &str,
    ) -> ara_ui::Result<()> {
        let doc = self
            .resource_for(editor, "vfx")
            .ok_or("open an effect first")?;
        let parts = item.split('|').collect::<Vec<_>>();
        let [emitter_name, stage, name] = parts.as_slice() else {
            return Err("invalid module address".into());
        };
        let graph = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(&doc)
                .ok_or("effect missing")?,
        )
        .map_err(|d| d.to_string())?;
        let effect = ara_vfx::Effect::from_graph(&graph).map_err(|d| d.to_string())?;
        let emitter = effect
            .emitters
            .iter()
            .find(|e| e.name == *emitter_name)
            .ok_or("emitter missing")?;
        let stage_value: ara_vfx::Stage = serde_json::from_value(json!(stage))?;
        let modules = match stage_value {
            ara_vfx::Stage::Spawn => &emitter.stacks.spawn,
            ara_vfx::Stage::Initialize => &emitter.stacks.initialize,
            ara_vfx::Stage::Update => &emitter.stacks.update,
            ara_vfx::Stage::Render => &emitter.stacks.render,
        };
        let index = modules
            .iter()
            .position(|m| m.name == *name)
            .ok_or("module missing")?;
        let args = match operation {
            "up" => {
                json!({"emitter":emitter_name,"stage":stage,"name":name,"operation":"move","index":index.saturating_sub(1)})
            }
            "down" => {
                json!({"emitter":emitter_name,"stage":stage,"name":name,"operation":"move","index":(index + 1).min(modules.len() - 1)})
            }
            "duplicate" => {
                let mut copy = modules[index].clone();
                copy.name = format!("{}_copy{}", copy.name, editor.host.store().revision());
                json!({"emitter":emitter_name,"stage":stage,"name":copy.name,"operation":"insert","index":index + 1,"module":copy})
            }
            _ => json!({"emitter":emitter_name,"stage":stage,"name":name,"operation":"remove"}),
        };
        editor.apply_to(
            &doc,
            vec![json!({"cmd":"vfx.stack.edit","args":args})],
            &format!("{} VFX module", super::inspector_panel::sentence(operation)),
        )?;
        Ok(())
    }
}
fn default_module(stage: ara_vfx::Stage, revision: u64) -> ara_vfx::StackModule {
    let module = match stage {
        ara_vfx::Stage::Spawn => ara_vfx::Module::Rate { per_second: 20. },
        ara_vfx::Stage::Initialize => ara_vfx::Module::Sphere {
            center: [0., 0., 0.],
            radius: 0.2,
        },
        ara_vfx::Stage::Update => ara_vfx::Module::Drag { coefficient: 0.1 },
        ara_vfx::Stage::Render => ara_vfx::Module::Appearance {
            texture: ara_vfx::SpriteTexture::Fire,
            blend: ara_vfx::Blend::Additive,
            alignment: ara_vfx::Alignment::Camera,
            stretch: 1.,
            shutter: 0.,
        },
    };
    ara_vfx::StackModule {
        name: format!("module{revision}"),
        module,
        bindings: Default::default(),
    }
}
