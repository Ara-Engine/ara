//! Ordered emitter/module stacks. Graph topology is never shown as a VFX UI.
use super::*;
impl Shell {
    pub(super) fn vfx_panel(&mut self, editor: &Editor, out: &mut String) -> ara_ui::Result<()> {
        let resource = self.resource_for(editor, "vfx");
        self.resource_toolbar(editor, out, "vfx", resource.as_deref());
        let Some(doc) = resource else {
            out.push_str("<panel id='vfx-empty' class='body'><text id='vfx-welcome' text='Create or open an effect with New.' /></panel>");
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
        out.push_str("<panel id='vfx-content' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px'><scroll id='vfx-stack-list' style='width:316px;flex-basis:316px;flex-shrink:0;padding:12px;gap:12px'>");
        for emitter in &effect.emitters {
            out.push_str(&format!("<panel id='emitter-{}' class='row'>{}<text id='emitter-title-{}' class='heading' text='{}' /><panel id='emitter-fill-{}' style='flex-grow:1' /><text id='emitter-capacity-{}' class='badge' text='{} max' /></panel>",escape(&emitter.name),icons::icon(&emitter.name,"layers"),escape(&emitter.name),escape(&emitter.name),escape(&emitter.name),escape(&emitter.name),emitter.capacity));
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
                out.push_str(&format!("<panel id='stage-{}-{stage_name}' style='gap:4px'><text id='stage-label-{}-{stage_name}' class='field-label' text='{}' style='padding:4px' />",emitter.name,emitter.name,stage_name.to_uppercase()));
                for (index, module) in modules.iter().enumerate() {
                    let item = format!("{}|{stage_name}|{}", emitter.name, module.name);
                    let id = format!("module-{}-{stage_name}-{}", emitter.name, module.name);
                    let value = ara_json::to_value(&module.module)?;
                    let kind = value["op"].as_str().unwrap_or("module");
                    out.push_str(&format!(
                        "<panel id='{id}-card' class='module-card {}'>",
                        if editor.state.resource_item.as_deref() == Some(&item) {
                            "module-selected"
                        } else {
                            ""
                        }
                    ));
                    out.push_str(&self.bind_button(
                        &id,
                        &format!("{}  /  {}", module.name, kind),
                        Binding::Protocol("editor.workspace", json!({"resource":doc,"item":item})),
                        "module-title",
                    ));
                    out.push_str(&format!(
                        "<panel id='{id}-actions' class='row' style='min-height:22px;gap:4px'>"
                    ));
                    for (action, label, target) in [
                        ("up", "Up", index.saturating_sub(1)),
                        (
                            "down",
                            "Down",
                            (index + 1).min(modules.len().saturating_sub(1)),
                        ),
                    ] {
                        out.push_str(&self.bind_button(&format!("{id}-{action}"),label,Binding::ResourceContent(doc.clone(),vec![json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter.name,"stage":stage_name,"name":module.name,"operation":"move","index":target}})],"Reorder VFX module".into()),"small quiet"));
                    }
                    let mut copy = module.clone();
                    copy.name = format!("{}_copy{}", copy.name, editor.host.store().revision());
                    out.push_str(&self.bind_button(&format!("{id}-copy"),"Duplicate",Binding::ResourceContent(doc.clone(),vec![json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter.name,"stage":stage_name,"name":copy.name,"operation":"insert","index":index+1,"module":copy}})],"Duplicate VFX module".into()),"small quiet"));
                    out.push_str(&self.bind_button(&format!("{id}-remove"),"Remove",Binding::ResourceContent(doc.clone(),vec![json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter.name,"stage":stage_name,"name":module.name,"operation":"remove"}})],"Remove VFX module".into()),"small quiet"));
                    out.push_str("</panel></panel>");
                }
                let module = default_module(stage, editor.host.store().revision());
                out.push_str(&self.bind_button(&format!("module-add-{}-{stage_name}",emitter.name),"+ Add module",Binding::ResourceContent(doc.clone(),vec![json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter.name,"stage":stage_name,"name":module.name,"operation":"insert","index":modules.len(),"module":module}})],"Add VFX module".into()),"small quiet"));
                out.push_str("</panel>");
            }
        }
        out.push_str("</scroll><panel id='vfx-preview-area' style='flex-grow:1;flex-basis:0px;padding:12px;gap:12px;background:var(--background)'><panel id='vfx-preview-tools' class='row'><text id='vfx-preview-title' class='heading' text='Live preview' /><panel id='vfx-preview-fill' style='flex-grow:1' />");
        out.push_str(&self.bind_button(
            "vfx-preview-play",
            if editor.state.preview_playing {
                "Pause"
            } else {
                "Play"
            },
            Binding::VfxPlayback(!editor.state.preview_playing),
            "small quiet",
        ));
        out.push_str("</panel>");
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
        out.push_str(&format!("<text id='vfx-preview-caption' class='muted' text='{} / {} Hz / fixed ticks' /><text id='vfx-preview-detail' class='badge' text='Changes compile in the background. The last good preview stays visible.' style='white-space:normal' />",escape(&effect.name),effect.hz));
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
