//! The VFX editor (Niagara / VFX Graph style): emitters as ordered module stacks
//! (spawn, initialize, update, render) on the left, the live GPU preview with its
//! timeline (restart, play/pause, step, rate, time, particles) in the middle, the
//! selected module in the Inspector, and the compile status along the bottom.
//! Hide and Isolate are preview-only; they never change the effect.
use super::commands::Target;
use super::*;

/// Width of the emitter stack column, in logical pixels.
const STACK: f32 = 320.;
/// Timeline rates, in percent of real time.
pub(super) const RATES: &[u32] = &[25, 50, 100, 200];

/// The preview's latest GPU counters (updated in place, every preview frame).
#[derive(Default)]
pub(crate) struct VfxLive {
    /// Completed ticks since the (re)start.
    pub tick: u32,
    /// Live particles, all emitters.
    pub alive: u32,
    /// Live particles per emitter.
    pub emitters: Vec<(String, u32)>,
}

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
        self.vfx_hz = effect.hz;
        self.vfx_stream.show(
            self.preview_effect(&effect),
            editor.state.preview_playing,
            self.vfx_rate,
        );
        out.push_str("<panel id='vfx-content' class='tool-body'>");
        out.push_str(&format!(
            "<panel id='vfx-side' class='tool-side' style='width:{STACK}px'>"
        ));
        out.push_str(&format!("<panel id='vfx-emitters-header' class='side-header'><text id='vfx-emitters-title' class='heading' text='Emitters' /><text id='vfx-emitters-count' class='caption' text='{}' /><panel id='vfx-emitters-fill' class='fill' /></panel>",effect.emitters.len()));
        out.push_str("<scroll id='vfx-stack-list' style='flex-grow:1;flex-basis:0px;min-height:0px;padding:4px;padding-top:0px;gap:0px'>");
        for emitter in &effect.emitters {
            self.emitter_header(emitter, out);
            if self.vfx_collapsed.contains(&emitter.name) {
                continue;
            }
            self.emitter_stack(editor, &doc, emitter, out)?;
        }
        out.push_str("</scroll></panel><panel id='vfx-side-edge' class='tool-edge' />");
        out.push_str("<panel id='vfx-preview-area' class='tool-main' style='padding:8px;gap:8px'>");
        if self.vfx_ready {
            let size = self
                .ui
                .find("vfx-preview-area")
                .map(|i| {
                    let r = self.ui.nodes[i].rect;
                    (r.w - 24.).min(r.h - 64.)
                })
                .unwrap_or(320.)
                .max(96.);
            out.push_str(&format!("<panel id='vfx-live-frame' style='flex-grow:1;flex-basis:0px;min-height:0px;align-items:center;justify-content:center;overflow:hidden'><image id='vfx-live-image' src='vfx-live-preview' style='width:{size}px;height:{size}px' label='Live GPU particle preview' /></panel>"));
        } else {
            out.push_str("<panel id='vfx-preview-loading' style='flex-grow:1;flex-basis:0px;align-items:center;justify-content:center'><text id='vfx-preview-loading-title' class='muted' text='Compiling the effect…' /></panel>");
        }
        self.vfx_timeline(editor, out);
        out.push_str("</panel></panel>");
        self.vfx_status(&effect, out);
        Ok(())
    }

    /// The effect the preview runs: hidden emitters removed, or only the isolated one.
    fn preview_effect(&self, effect: &ara_vfx::Effect) -> ara_vfx::Effect {
        let mut shown = effect.clone();
        shown.emitters.retain(|e| match &self.vfx_solo {
            Some(solo) => &e.name == solo,
            None => !self.vfx_hidden.contains(&e.name),
        });
        if shown.emitters.is_empty() {
            return effect.clone();
        }
        shown
    }

    /// An emitter's header: collapse, name, live particles / capacity, Hide, Isolate.
    fn emitter_header(&mut self, emitter: &ara_vfx::Emitter, out: &mut String) {
        let name = &emitter.name;
        let collapsed = self.vfx_collapsed.contains(name);
        let hidden = self.vfx_hidden.contains(name);
        let solo = self.vfx_solo.as_deref() == Some(name.as_str());
        let muted = hidden || self.vfx_solo.as_ref().is_some_and(|s| s != name);
        let alive = self
            .vfx_live
            .as_ref()
            .and_then(|l| l.emitters.iter().find(|(n, _)| n == name))
            .map_or(0, |(_, a)| *a);
        let id = format!("emitter-{name}");
        out.push_str(&format!(
            "<panel id='{id}' class='emitter-header{}'>",
            if muted { " emitter-muted" } else { "" }
        ));
        let collapse = format!("{id}-collapse");
        let (icon, label) = if collapsed {
            ("chevron-right", "Expand")
        } else {
            ("chevron-down", "Collapse")
        };
        self.bind_command(
            &collapse,
            "vfx.emitter.collapse",
            Target::Text(name.clone()),
        );
        out.push_str(&format!("<button id='{collapse}' class='ghost icon-button emitter-tool' label='{label}' description='{label} the emitter' action='Dispatch'>{}</button>",icons::icon(&collapse,icon)));
        out.push_str(&format!("{}<text id='emitter-title-{name}' class='heading' text='{}' /><panel id='emitter-fill-{name}' class='fill' /><text id='emitter-alive-{name}' class='caption mono' text='{alive} / {}' />",icons::icon(&id,"sparkles"),escape(name),emitter.capacity));
        for (button, command, icon, label, on) in [
            (
                format!("{id}-hide"),
                "vfx.emitter.hide",
                if hidden { "eye-off" } else { "eye" },
                if hidden {
                    "Show in the preview"
                } else {
                    "Hide in the preview"
                },
                hidden,
            ),
            (
                format!("{id}-isolate"),
                "vfx.emitter.isolate",
                "scan-eye",
                if solo {
                    "Show every emitter"
                } else {
                    "Isolate in the preview"
                },
                solo,
            ),
        ] {
            self.bind_command(&button, command, Target::Text(name.clone()));
            out.push_str(&format!("<button id='{button}' class='{} icon-button emitter-tool' label='{label}' description='{label} (preview only)' action='Dispatch'>{}</button>",if on{"selected"}else{"ghost"},if on{icons::icon_in(&button,icon,"on-selection")}else{icons::icon(&button,icon)}));
        }
        out.push_str("</panel>");
    }

    /// An emitter's four stages and their modules.
    fn emitter_stack(
        &mut self,
        editor: &Editor,
        doc: &str,
        emitter: &ara_vfx::Emitter,
        out: &mut String,
    ) -> ara_ui::Result<()> {
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
            self.bindings.insert(ara_ui::source::stable_id(&add), Binding::ResourceContent(doc.into(),vec![json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter.name,"stage":stage_name,"name":module.name,"operation":"insert","index":modules.len(),"module":module}})],"Add VFX module".into()));
            out.push_str(&format!("<panel id='stage-{}-{stage_name}' class='stage-header'><panel id='stage-rail-{}-{stage_name}' class='stage-rail' style='background:var(--{rail})' /><text id='stage-label-{}-{stage_name}' class='caption' text='{}' /><panel id='stage-fill-{}-{stage_name}' class='fill' /><button id='{add}' class='ghost icon-button' label='Add module' description='Add a {} module' action='Dispatch'>{}</button></panel>",emitter.name,emitter.name,emitter.name,stage_name.to_uppercase(),emitter.name,stage_name,icons::icon(&add,"plus")));
            for module in modules.iter() {
                let item = format!("{}|{stage_name}|{}", emitter.name, module.name);
                let id = format!("module-{}-{stage_name}-{}", emitter.name, module.name);
                let value = ara_json::to_value(&module.module)?;
                let kind = value["op"].as_str().unwrap_or("module");
                let selected = editor.state.resource_item.as_deref() == Some(&item);
                // The key values that fit (never a value cut mid-way); the Inspector
                // has the rest.
                let mut summary = String::new();
                for (k, v) in value
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(k, _)| *k != "op")
                {
                    let part = format!(
                        "{} {}",
                        super::inspector_panel::sentence(k).to_lowercase(),
                        short(v)
                    );
                    let joined = summary.chars().count() + part.chars().count() + 3;
                    if !summary.is_empty() && joined > 30 {
                        break;
                    }
                    if !summary.is_empty() {
                        summary.push_str(" · ");
                    }
                    summary.push_str(&part);
                    if summary.chars().count() > 30 {
                        break;
                    }
                }
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Protocol("editor.workspace", json!({"resource":doc,"item":item})),
                );
                // A named module reads as its name, its kind muted after it.
                let (title, kind_text) = if module.name == kind {
                    (super::inspector_panel::sentence(kind), String::new())
                } else {
                    (
                        module.name.clone(),
                        super::inspector_panel::sentence(kind).to_lowercase(),
                    )
                };
                let muted = if selected {
                    "on-selection-muted"
                } else {
                    "caption"
                };
                // A disabled module stays in place, dimmed, with its checkbox clear.
                let name_class = match (selected, module.enabled) {
                    (true, _) => "on-selection",
                    (false, true) => "fg",
                    (false, false) => "muted",
                };
                out.push_str(&format!("<panel id='{id}-card' class='module-row{}'><panel id='{id}-rail' class='stage-rail' style='background:var(--{rail})' /><button id='{id}' class='module-title' label='{}' description='{}' action='Dispatch'><text id='{id}-name' class='{name_class}' text='{}' /><text id='{id}-kind' class='{muted}' text='{}' /><panel id='{id}-gap' class='fill' /><text id='{id}-summary' class='{muted}' style='font-size:11px;flex-shrink:1;min-width:0px;overflow:hidden' text='{}' /></button>",if selected{" module-selected"}else{""},escape(&module.name),escape(ara_vfx::library::MODULES.iter().find(|k|k.name==kind).map(|k|k.doc).unwrap_or("Typed VFX module")),escape(&title),escape(&kind_text),escape(&if module.enabled{summary}else{"disabled".into()})));
                if selected {
                    for (command, icon, label) in [
                        ("vfx.module.up", "chevron-up", "Move up · Alt+Up"),
                        ("vfx.module.down", "chevron-down", "Move down · Alt+Down"),
                        ("vfx.module.duplicate", "copy", "Duplicate · Ctrl+D"),
                        ("vfx.module.delete", "trash", "Delete · Del"),
                    ] {
                        let button = format!("{id}-{}", command.rsplit('.').next().unwrap_or(""));
                        self.bind_command(&button, command, Target::Text(item.clone()));
                        out.push_str(&format!("<button id='{button}' class='ghost icon-button' style='width:20px;height:20px;padding:2px' label='{label}' description='{label}' action='Dispatch'>{}</button>",icons::icon_in(&button,icon,"on-selection")));
                    }
                }
                let enable = format!("{id}-enabled");
                self.bindings.insert(
                    ara_ui::source::stable_id(&enable),
                    Binding::ResourceContent(
                        doc.into(),
                        vec![json!({"cmd":"vfx.module.enable","args":{"emitter":emitter.name,"stage":stage_name,"name":module.name,"enabled":!module.enabled}})],
                        if module.enabled { "Disable VFX module" } else { "Enable VFX module" }.into(),
                    ),
                );
                out.push_str(&format!("<toggle id='{enable}' class='module-enable' checked='{}' label='Run {}' description='Run this module (unchecked: kept but skipped)' action='Dispatch' />",module.enabled,escape(&module.name)));
                out.push_str("</panel>");
            }
        }
        Ok(())
    }

    /// The timeline: restart, play/pause, step, time and tick, rate, particles.
    fn vfx_timeline(&mut self, editor: &Editor, out: &mut String) {
        let playing = editor.state.preview_playing;
        out.push_str("<panel id='vfx-timeline' class='timeline'>");
        self.tool_icon(
            out,
            "vfx-restart",
            "vfx.restart",
            "rotate-ccw",
            editor,
            false,
        );
        self.bindings.insert(
            ara_ui::source::stable_id("vfx-preview-play"),
            Binding::VfxPlayback(!playing),
        );
        let label = if playing { "Pause" } else { "Play" };
        out.push_str(&format!("<button id='vfx-preview-play' class='ghost icon-button' label='{label}' description='{label} the preview' action='Dispatch'>{}</button>",icons::icon("vfx-preview-play",if playing{"pause"}else{"play"})));
        self.tool_icon(out, "vfx-step", "vfx.step", "step-forward", editor, false);
        out.push_str(&format!(
            "<panel id='vfx-timeline-sep' class='toolbar-sep' /><text id='vfx-time' class='mono' text='{}' />",
            escape(&self.vfx_time_text())
        ));
        out.push_str(
            "<panel id='vfx-timeline-fill' class='fill' /><panel id='vfx-rate' class='segment'>",
        );
        for rate in RATES {
            let id = format!("vfx-rate-{rate}");
            self.bind_command(&id, "vfx.rate", Target::Text(rate.to_string()));
            let label = match rate {
                25 => "¼×".to_owned(),
                50 => "½×".to_owned(),
                r => format!("{}×", r / 100),
            };
            out.push_str(&format!("<button id='{id}' class='segment-button{}' label='{label}' description='Preview at {rate} % speed' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",if *rate==self.vfx_rate{" selected"}else{""}));
        }
        out.push_str(&format!(
            "</panel><text id='vfx-particles' class='caption mono' text='{}' /></panel>",
            escape(&self.vfx_particles_text())
        ));
    }

    /// Compile status: a failed compile keeps the last good preview running.
    fn vfx_status(&mut self, effect: &ara_vfx::Effect, out: &mut String) {
        out.push_str("<panel id='vfx-status' class='tool-status'>");
        if let Some(error) = &self.vfx_error {
            out.push_str(&format!("{}<text id='vfx-status-state' class='destructive' text='Compile failed' /><text id='vfx-status-error' class='fg' text='{} · the last good preview keeps running' />",icons::icon_in("vfx-status-icon","circle-x","destructive"),escape(error)));
        } else if !self.vfx_ready {
            out.push_str(&format!(
                "{}<text id='vfx-status-state' class='muted' text='Compiling…' />",
                icons::icon_in("vfx-status-icon", "refresh-cw", "muted")
            ));
        } else {
            out.push_str(&format!(
                "{}<text id='vfx-status-state' class='success' text='Compiled' />",
                icons::icon_in("vfx-status-icon", "circle-check", "success")
            ));
        }
        let capacity: u32 = effect.emitters.iter().map(|e| e.capacity).sum();
        let modules: usize = effect
            .emitters
            .iter()
            .map(|e| {
                e.stacks.spawn.len()
                    + e.stacks.initialize.len()
                    + e.stacks.update.len()
                    + e.stacks.render.len()
            })
            .sum();
        out.push_str(&format!("<panel id='vfx-status-fill' class='fill' /><text id='vfx-status-stats' class='caption' text='{} emitters · {modules} modules · capacity {capacity} · fixed {} Hz · seed {}' /></panel>",effect.emitters.len(),effect.hz,effect.seed));
    }

    fn vfx_time_text(&self) -> String {
        let tick = self.vfx_live.as_ref().map_or(0, |l| l.tick);
        let seconds = f64::from(tick) / f64::from(self.vfx_hz.max(1));
        format!("{seconds:6.2} s · tick {tick}")
    }

    fn vfx_particles_text(&self) -> String {
        format!(
            "{} particles",
            self.vfx_live.as_ref().map_or(0, |l| l.alive)
        )
    }

    /// Refreshes the timeline and emitter counters in place (no rebuild per frame).
    pub(super) fn vfx_live_text(&mut self) -> ara_ui::Result<()> {
        let mut texts = vec![
            ("vfx-time".to_owned(), self.vfx_time_text()),
            ("vfx-particles".to_owned(), self.vfx_particles_text()),
        ];
        if let Some(live) = &self.vfx_live {
            for (name, alive) in &live.emitters {
                let id = format!("emitter-alive-{name}");
                let capacity = self
                    .ui
                    .find(&id)
                    .and_then(|i| self.ui.nodes[i].text.as_ref())
                    .and_then(|t| t.value.rsplit(" / ").next().map(str::to_owned))
                    .unwrap_or_default();
                texts.push((id, format!("{alive} / {capacity}")));
            }
        }
        for (id, text) in texts {
            if let Some(i) = self.ui.find(&id) {
                self.ui.set_text(i, &text)?;
            }
        }
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
        let width = self
            .ui
            .find("vfx-inspector")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(300.);
        let label = self.label_px(width);
        out.push_str("<scroll id='vfx-inspector' style='flex-grow:1;flex-basis:0px;min-height:0px;padding-top:6px;padding-bottom:12px'>");
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
                    out.push_str(&format!("<panel id='vfx-module-header' class='component-header'>{}<text id='vfx-module-name' class='component-title' text='{}' /><text id='vfx-module-kind' class='caption' text='{}' /></panel><panel id='vfx-module-doc-row' style='flex-shrink:0;padding:6px;padding-left:12px'><text id='vfx-module-doc' class='dialog-text' text='{}' /></panel>",icons::icon("vfx-module-header","sparkles"),escape(&module.name),escape(&super::inspector_panel::sentence(kind).to_lowercase()),escape(ara_vfx::library::MODULES.iter().find(|k|k.name==kind).map(|k|k.doc).unwrap_or("Typed VFX module"))));
                    if let Some(fields) = value.as_object() {
                        for (key, v) in fields {
                            if key == "op" {
                                continue;
                            }
                            let id = format!("vfx-param-{key}");
                            out.push_str(&format!("<panel id='{id}-row' class='prop-row'><panel id='{id}-label-cell' class='prop-label' style='width:{label}px'><text id='{id}-label' text='{}' /></panel><panel id='{id}-value' class='prop-value'>",escape(&super::inspector_panel::sentence(key))));
                            self.bindings.insert(
                                ara_ui::source::stable_id(&id),
                                Binding::VfxField(doc.clone(), item.into(), key.clone()),
                            );
                            if let Some(v) = v.as_bool() {
                                out.push_str(&format!(
                                    "<toggle id='{id}' checked='{v}' action='Dispatch' />"
                                ));
                            } else {
                                out.push_str(&format!("<field id='{id}' class='number' style='flex-grow:1;flex-basis:0px' text='{}' label='{}' action='Dispatch' />",escape(&panels::display(v)),escape(key)));
                            }
                            out.push_str("</panel></panel>");
                        }
                    }
                }
            }
        } else {
            self.empty(
                out,
                "vfx-module-empty",
                "sparkles",
                "No module selected",
                "Click a module in the stack to edit its parameters.",
                &[],
            );
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
        Value::Array(a) if a.iter().any(|v| v.is_object() || v.is_array()) => {
            format!("{} keys", a.len())
        }
        Value::Array(a) => a.iter().map(short).collect::<Vec<_>>().join(", "),
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
            "toggle" => {
                let enabled = !modules[index].enabled;
                editor.apply_to(
                    &doc,
                    vec![json!({"cmd":"vfx.module.enable","args":{"emitter":emitter_name,"stage":stage,"name":name,"enabled":enabled}})],
                    if enabled { "Enable VFX module" } else { "Disable VFX module" },
                )?;
                return Ok(());
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
    ara_vfx::StackModule::new(format!("module{revision}"), module)
}
