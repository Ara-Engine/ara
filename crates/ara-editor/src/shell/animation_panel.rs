//! State machines and typed pose/blend-space views over the same animation controller.
use super::*;
use ara_anim::graph::{BlendSample, ControllerData, Node, NodeKind, Parameter, State, Transition};
impl Shell {
    pub(super) fn animation_panel(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let resource = self.resource_for(editor, "animation");
        out.push_str("<panel id='animation-toolbar' class='toolbar'><panel id='animation-view' class='segment'>");
        for (id, label, blends) in [
            ("animation-states", "States", false),
            ("animation-blends", "Blend spaces", true),
        ] {
            self.bindings.insert(
                ara_ui::source::stable_id(id),
                Binding::Protocol("editor.workspace", json!({"blends":blends})),
            );
            out.push_str(&format!("<button id='{id}' class='segment-button{}' label='{label}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",if editor.state.animation_blends == blends {" selected"} else {""}));
        }
        out.push_str("</panel><panel id='animation-sep' class='toolbar-sep' />");
        for (id, label, kind) in [
            ("animation-add-state", "State", "state"),
            ("animation-add-blend", "Blend", "blend"),
            ("animation-add-transition", "Transition", "transition"),
        ] {
            self.bindings.insert(
                ara_ui::source::stable_id(id),
                Binding::AnimationEdit(kind.into()),
            );
            out.push_str(&format!("<button id='{id}' class='ghost' label='Add {label}' description='Add a {}' action='Dispatch'>{}<text id='{id}-label' text='{label}' /></button>",label.to_lowercase(),icons::icon(id,"plus")));
        }
        out.push_str("<panel id='animation-toolbar-fill' class='fill' />");
        self.tool_text(
            out,
            "resource-create-animation",
            "assets.create.animation",
            "plus",
            "New",
            "ghost",
        );
        out.push_str("</panel>");
        let Some(doc) = resource else {
            self.empty(
                out,
                "animation-empty",
                "person-standing",
                "No controller open",
                "Open an animation controller from Assets, or create one.",
                &[(
                    "animation-empty-new",
                    "New controller",
                    "assets.create.animation",
                )],
            );
            return Ok(());
        };
        let document = editor
            .host
            .store()
            .workspace()
            .doc(&doc)
            .ok_or("controller missing")?;
        let (_, controller) = crate::authoring::animation::controller(document)?;
        let path = crate::authoring::animation::layout_path(&doc);
        let layout = editor
            .host
            .store()
            .workspace()
            .find(&path)
            .and_then(|id| editor.host.store().workspace().doc(id.as_str()))
            .and_then(|d| ara_graph::Graph::from_document(d).ok())
            .map(|g| g.editor)
            .unwrap_or_default();
        let changed = self.canvas.as_ref().is_none_or(|c| {
            c.graph().header.payload["blends"] != editor.state.animation_blends
                || c.graph().editor != layout
        });
        if self.canvas_doc.as_deref() != Some(&doc)
            || self.canvas_revision != editor.host.store().revision()
            || changed
        {
            let graph = crate::authoring::animation::projection(
                document,
                layout,
                editor.state.animation_blends,
            )?;
            if let Some(canvas) = self
                .canvas
                .as_mut()
                .filter(|_| self.canvas_doc.as_deref() == Some(&doc))
            {
                canvas.refresh(graph, &mut self.text);
            } else {
                self.canvas = Some(crate::graph::Canvas::new(graph, &mut self.text));
            }
            self.canvas_doc = Some(doc.clone());
            self.canvas_revision = editor.host.store().revision();
        }
        self.apply_canvas_view(editor);
        out.push_str("<panel id='graph-canvas' style='flex-grow:1;flex-basis:0px;background:var(--background);overflow:hidden' label='Animation controller canvas' /><panel id='animation-transition-list' style='height:112px;flex-shrink:0;padding:8px;gap:4px;background:var(--card)'><text id='animation-transition-heading' class='field-label' text='Transitions / priority order' /><scroll id='animation-transitions' style='flex-grow:1;flex-basis:0px;gap:2px'>");
        for transition in &controller.transitions {
            out.push_str(&self.bind_button(
                &format!("transition-{}", transition.id),
                &format!(
                    "{} -> {}    {} ticks",
                    transition.from, transition.to, transition.duration
                ),
                Binding::Protocol(
                    "editor.workspace",
                    json!({"item":format!("transition:{}",transition.id)}),
                ),
                "small quiet folder-row",
            ));
        }
        out.push_str("</scroll></panel>");
        Ok(())
    }
    pub(super) fn edit_animation(&mut self, editor: &mut Editor, kind: &str) -> ara_ui::Result<()> {
        let Some(doc) = self.resource_for(editor, "animation") else {
            return Ok(());
        };
        let (record, mut controller) = crate::authoring::animation::controller(
            editor
                .host
                .store()
                .workspace()
                .doc(&doc)
                .ok_or("controller missing")?,
        )?;
        let n = editor.host.store().revision();
        match kind {
            "state" => {
                let name = format!("State{}", controller.states.len() + 1);
                let pose = format!("clip{n}");
                controller.nodes.insert(pose.clone(), Node::default());
                controller.states.insert(
                    name.clone(),
                    State {
                        node: pose,
                        ..Default::default()
                    },
                );
                if controller.initial.is_empty() {
                    controller.initial = name;
                }
            }
            "blend" => {
                let mut children = controller.nodes.keys().take(2).cloned().collect::<Vec<_>>();
                while children.len() < 2 {
                    let key = format!("clip{n}_{}", children.len());
                    controller.nodes.insert(key.clone(), Node::default());
                    children.push(key);
                }
                let name = format!("Locomotion{}", controller.nodes.len());
                controller
                    .parameters
                    .entry("speed".into())
                    .or_insert_with(Parameter::default);
                controller.nodes.insert(
                    name.clone(),
                    Node {
                        kind: NodeKind::Blend1d,
                        parameter_x: "speed".into(),
                        samples: children
                            .into_iter()
                            .enumerate()
                            .map(|(i, node)| BlendSample {
                                node,
                                x: i as f32,
                                y: 0.,
                            })
                            .collect(),
                        ..Default::default()
                    },
                );
                controller.states.insert(
                    name.clone(),
                    State {
                        node: name.clone(),
                        ..Default::default()
                    },
                );
                if controller.initial.is_empty() {
                    controller.initial = name;
                }
            }
            "transition" => {
                let states = controller
                    .states
                    .keys()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>();
                if let [from, to] = states.as_slice() {
                    controller.transitions.push(Transition {
                        id: format!("transition{n}"),
                        from: from.clone(),
                        to: to.clone(),
                        duration: 4800,
                        exit_tick: None,
                        conditions: Vec::new(),
                    });
                }
            }
            _ => {}
        }
        self.save_controller(editor, &doc, &record, controller, "Edit animation graph")?;
        Ok(())
    }
    fn save_controller(
        &self,
        editor: &mut Editor,
        doc: &str,
        record: &str,
        controller: ControllerData,
        why: &str,
    ) -> ara_ui::Result<()> {
        editor.apply_to(doc,vec![json!({"cmd":"editor.animation.controller.set","args":{"record":record,"controller":controller}})],why)?;
        Ok(())
    }
    pub(super) fn animation_connect(
        &self,
        editor: &mut Editor,
        from: ara_graph::Endpoint,
        to: ara_graph::Endpoint,
    ) -> ara_ui::Result<()> {
        let (Some(doc), Some(canvas)) = (&self.canvas_doc, &self.canvas) else {
            return Ok(());
        };
        let (record, mut controller) = crate::authoring::animation::controller(
            editor
                .host
                .store()
                .workspace()
                .doc(doc)
                .ok_or("controller missing")?,
        )?;
        let source = canvas
            .graph()
            .nodes
            .get(&from.node)
            .ok_or("source node missing")?;
        let target = canvas
            .graph()
            .nodes
            .get(&to.node)
            .ok_or("target node missing")?;
        if let (Some(from), Some(to)) = (
            source.payload["state"].as_str(),
            target.payload["state"].as_str(),
        ) {
            controller.transitions.push(Transition {
                id: format!("transition{}", editor.host.store().revision()),
                from: from.into(),
                to: to.into(),
                duration: 4800,
                exit_tick: None,
                conditions: Vec::new(),
            });
        } else if let (Some(from), Some(target)) = (
            source.payload["pose"].as_str(),
            target.payload["pose"].as_str(),
        ) {
            let node = controller
                .nodes
                .get_mut(target)
                .ok_or("pose node missing")?;
            let index = to
                .port
                .strip_prefix("sample")
                .and_then(|p| p.parse::<usize>().ok())
                .ok_or("invalid blend input")?;
            node.samples
                .get_mut(index)
                .ok_or("blend sample missing")?
                .node = from.into();
        }
        self.save_controller(editor, doc, &record, controller, "Connect animation graph")
    }
    pub(super) fn animation_inspector(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let Some(doc) = self.resource_for(editor, "animation") else {
            return Ok(());
        };
        let (_, controller) = crate::authoring::animation::controller(
            editor
                .host
                .store()
                .workspace()
                .doc(&doc)
                .ok_or("controller missing")?,
        )?;
        out.push_str("<scroll id='animation-inspector' class='body'><text id='animation-inspector-title' class='heading' text='Controller' />");
        if self.annotation_inspector(editor, out) {
            out.push_str("</scroll>");
            return Ok(());
        }

        if let Some(id) = editor
            .state
            .resource_item
            .as_deref()
            .and_then(|s| s.strip_prefix("transition:"))
        {
            if let Some(t) = controller.transitions.iter().find(|t| t.id == id) {
                out.push_str(&format!(
                    "<text id='transition-title' text='{} -> {}' style='white-space:normal' />",
                    escape(&t.from),
                    escape(&t.to)
                ));
                self.anim_field(
                    out,
                    &doc,
                    &format!("transition:{id}"),
                    "duration",
                    "Crossfade (ticks)",
                    &json!(t.duration),
                );
                self.anim_field(
                    out,
                    &doc,
                    &format!("transition:{id}"),
                    "exitTick",
                    "Exit tick",
                    &json!(t.exit_tick),
                );
                out.push_str(&format!("<text id='transition-conditions' class='badge' text='{} conditions / priority {}' />",t.conditions.len(),controller.transitions.iter().position(|x|x.id==id).unwrap_or(0)));
            }
        } else if let Some(node) = self.canvas.as_ref().and_then(|c| {
            editor
                .state
                .canvas_selection
                .iter()
                .next()
                .and_then(|id| c.graph().nodes.get(id))
                .cloned()
        }) {
            if let Some(state) = node.payload["state"]
                .as_str()
                .and_then(|id| controller.states.get_key_value(id))
            {
                let (key, value) = state;
                let item = format!("state:{key}");
                out.push_str(&format!(
                    "<text id='animation-state-title' style='font-size:16px' text='{}' />",
                    escape(key)
                ));
                self.anim_field(
                    out,
                    &doc,
                    &item,
                    "duration",
                    "Duration (ticks)",
                    &json!(value.duration),
                );
                self.anim_field(
                    out,
                    &doc,
                    &item,
                    "looping",
                    "Looping",
                    &json!(value.looping),
                );
                self.anim_field(
                    out,
                    &doc,
                    &item,
                    "syncGroup",
                    "Sync group",
                    &json!(value.sync_group),
                );
                self.anim_field(out, &doc, &item, "node", "Pose node", &json!(value.node));
            }
            if let Some(key) = node.payload["pose"].as_str()
                && let Some(value) = controller.nodes.get(key)
            {
                let item = format!("pose:{key}");
                out.push_str("<panel id='animation-pose-divider' class='divider' />");
                self.anim_field(
                    out,
                    &doc,
                    &item,
                    "kind",
                    "Pose operation",
                    &json!(value.kind),
                );
                if value.kind == NodeKind::Clip {
                    self.anim_field(out, &doc, &item, "clip", "Clip asset", &json!(value.clip));
                } else {
                    self.anim_field(
                        out,
                        &doc,
                        &item,
                        "parameterX",
                        "Horizontal parameter",
                        &json!(value.parameter_x),
                    );
                    if value.kind == NodeKind::Directional2d {
                        self.anim_field(
                            out,
                            &doc,
                            &item,
                            "parameterY",
                            "Vertical parameter",
                            &json!(value.parameter_y),
                        );
                    }
                    out.push_str("<panel id='blend-space-plot' style='height:128px;flex-shrink:0;background:var(--background);border-width:1px;border-color:var(--border);border-radius:var(--radius-md);overflow:hidden'><panel id='blend-axis-x' style='position:absolute;left:8px;top:64px;width:160px;height:1px;background:var(--border)' /><panel id='blend-axis-y' style='position:absolute;left:84px;top:8px;width:1px;height:112px;background:var(--border)' />");
                    for (n, sample) in value.samples.iter().enumerate() {
                        out.push_str(&format!("<panel id='blend-point-{n}' style='position:absolute;left:{}px;top:{}px;width:8px;height:8px;border-radius:4px;background:var(--primary)' description='{}: ({}, {})' />",12.+sample.x.clamp(-1.,1.)*70.+70.,60.-sample.y.clamp(-1.,1.)*48.,escape(&sample.node),sample.x,sample.y));
                    }
                    out.push_str("</panel>");
                    for (n, sample) in value.samples.iter().enumerate() {
                        out.push_str(&format!(
                            "<text id='blend-sample-{n}' class='field-label' text='{}' />",
                            escape(&sample.node)
                        ));
                        self.anim_field(
                            out,
                            &doc,
                            &item,
                            &format!("sample{n}x"),
                            "X",
                            &json!(sample.x),
                        );
                        if value.kind == NodeKind::Directional2d {
                            self.anim_field(
                                out,
                                &doc,
                                &item,
                                &format!("sample{n}y"),
                                "Y",
                                &json!(sample.y),
                            );
                        }
                    }
                }
            }
        } else {
            out.push_str("<text id='animation-select-help' class='muted' style='white-space:normal' text='Select a state or pose node to edit it. Drag between state ports to add a transition.' />");
        }
        out.push_str("</scroll>");
        Ok(())
    }
    fn anim_field(
        &mut self,
        out: &mut String,
        doc: &str,
        item: &str,
        key: &str,
        label: &str,
        value: &Value,
    ) {
        let id = if key == "clip" {
            "animation-clip-field".into()
        } else {
            format!("animation-field-{key}")
        };
        self.bindings.insert(
            ara_ui::source::stable_id(&id),
            Binding::AnimationField(doc.into(), item.into(), key.into()),
        );
        out.push_str(&format!("<panel id='{id}-row' class='field-row'><text id='{id}-label' class='field-label' text='{}' />",escape(label)));
        if let Some(v) = value.as_bool() {
            out.push_str(&format!(
                "<toggle id='{id}' checked='{v}' action='Dispatch' />"
            ));
        } else {
            out.push_str(&format!(
                "<field id='{id}' text='{}' label='{}' action='Dispatch' />",
                escape(&panels::display(value)),
                escape(label)
            ));
        }
        out.push_str("</panel>");
    }
    pub(super) fn set_animation_field(
        &self,
        editor: &mut Editor,
        doc: &str,
        item: &str,
        key: &str,
        value: Value,
    ) -> ara_ui::Result<()> {
        let (record, mut controller) = crate::authoring::animation::controller(
            editor
                .host
                .store()
                .workspace()
                .doc(doc)
                .ok_or("controller missing")?,
        )?;
        let (kind, name) = item.split_once(':').ok_or("animation field address")?;
        match kind {
            "state" => {
                let state = controller.states.get_mut(name).ok_or("state missing")?;
                let mut source = ara_json::to_value(&*state)?;
                source[key] = value;
                *state = serde_json::from_value(source)?;
            }
            "transition" => {
                let transition = controller
                    .transitions
                    .iter_mut()
                    .find(|t| t.id == name)
                    .ok_or("transition missing")?;
                let mut source = ara_json::to_value(&*transition)?;
                source[key] = value;
                *transition = serde_json::from_value(source)?;
            }
            "pose" => {
                let node = controller.nodes.get_mut(name).ok_or("pose node missing")?;
                if let Some(sample) = key.strip_prefix("sample") {
                    let (index, axis) = sample.split_at(sample.len().saturating_sub(1));
                    let sample = node
                        .samples
                        .get_mut(index.parse::<usize>()?)
                        .ok_or("sample missing")?;
                    let value = value.as_f64().ok_or("sample must be numeric")? as f32;
                    if axis == "x" {
                        sample.x = value;
                    } else {
                        sample.y = value;
                    }
                } else {
                    let mut source = ara_json::to_value(&*node)?;
                    source[key] = value;
                    *node = serde_json::from_value(source)?;
                }
            }
            _ => return Err("unknown animation field".into()),
        }
        self.save_controller(editor, doc, &record, controller, "Edit animation parameter")
    }
}
