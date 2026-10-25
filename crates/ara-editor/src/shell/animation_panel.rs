//! The Animation page (Unity Animator / UE state machine): parameters and the
//! transitions in priority order on the left, the state machine (or pose graph)
//! canvas in the middle, the selection in the Inspector and a status strip.
//! Every edit replaces the controller through `editor.animation.controller.set`.
//! During Play the page follows a running animator ([`AnimationLive`]): the playing
//! state lights up on the canvas and parameters show their live values.
use super::*;
use ara_anim::graph::{
    BlendSample, ControllerData, Node, NodeKind, Parameter, ParameterKind, State, Transition,
};

/// Width of the parameters + transitions column, in logical pixels.
const SIDE: f32 = 240.;
/// How often the page reads the running animator during Play.
const LIVE_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// The running animator the Animation page follows during Play: the first scene
/// entity (the selected one first) whose `Animator.controller` is the open controller,
/// read through `play.read <entity>.AnimatorState`.
#[derive(Default)]
pub(crate) struct AnimationLive {
    polled: Option<std::time::Instant>,
    /// Scene revision and controller the entity was found for.
    found: Option<(u64, String)>,
    /// The followed entity's address.
    entity: Option<String>,
    /// The latest `AnimatorState` (`ara_anim::Status` as JSON).
    status: Option<Value>,
}

/// What a live poll changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveChange {
    /// Only the canvas moved (state progress): repaint.
    Canvas,
    /// The playing state, a parameter or the followed entity changed: rebuild.
    Page,
}

impl Shell {
    pub(super) fn animation_panel(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let resource = self.resource_for(editor, "animation");
        out.push_str("<panel id='animation-toolbar' class='toolbar'>");
        let path = resource
            .as_deref()
            .and_then(|d| editor.host.store().workspace().path_of(d));
        super::graph_panel::tool_crumbs(out, "animation", path);
        if resource.is_some() {
            out.push_str("<panel id='animation-view-sep' class='toolbar-sep' /><panel id='animation-view' class='segment'>");
            for (id, label, blends) in [
                ("animation-states", "States", false),
                ("animation-blends", "Blend spaces", true),
            ] {
                self.bind_command(
                    id,
                    if blends {
                        "animation.view_blends"
                    } else {
                        "animation.view_states"
                    },
                    super::commands::Target::None,
                );
                out.push_str(&format!("<button id='{id}' class='segment-button{}' label='{label}' description='Show the controller as {}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",if editor.state.animation_blends == blends {" selected"} else {""},if blends{"its pose and blend graph"}else{"a state machine"}));
            }
            out.push_str("</panel><panel id='animation-sep' class='toolbar-sep' />");
            for (id, label, kind, what) in [
                (
                    "animation-add-state",
                    "State",
                    "state",
                    "a state with a clip pose",
                ),
                (
                    "animation-add-blend",
                    "Blend",
                    "blend",
                    "a 1D blend state on the speed parameter",
                ),
                (
                    "animation-add-transition",
                    "Transition",
                    "transition",
                    "a transition between the first two states",
                ),
            ] {
                self.bindings.insert(
                    ara_ui::source::stable_id(id),
                    Binding::AnimationEdit(kind.into()),
                );
                out.push_str(&format!("<button id='{id}' class='ghost' label='Add {}' description='Add {what}' action='Dispatch'>{}<text id='{id}-label' text='{label}' /></button>",label.to_lowercase(),icons::icon(id,"plus")));
            }
            out.push_str("<panel id='animation-tools-sep' class='toolbar-sep' />");
            self.tool_icon(
                out,
                "animation-frame",
                "graph.frame",
                "focus",
                editor,
                false,
            );
        }
        out.push_str("<panel id='animation-toolbar-fill' class='fill' /></panel>");
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
        let revision = editor.host.store().revision();
        let current = self.graphs.get("animation").is_some_and(|g| {
            g.doc == doc
                && g.revision == revision
                && g.canvas.graph().header.payload["blends"] == editor.state.animation_blends
                && g.canvas.graph().editor == layout
        });
        if !current {
            let graph = crate::authoring::animation::projection(
                document,
                layout,
                editor.state.animation_blends,
            )?;
            self.set_graph("animation", &doc, graph, revision);
        }
        self.apply_canvas_view(editor);
        // A transition picked in the sidebar draws as selected on the canvas.
        let picked = editor
            .state
            .resource_item
            .as_deref()
            .and_then(|s| s.strip_prefix("transition:"));
        let live = self
            .animation_live
            .status
            .as_ref()
            .filter(|_| !editor.state.animation_blends)
            .and_then(|status| crate::authoring::animation::live(&controller, status));
        if let Some(canvas) = self.graphs.get_mut("animation").map(|g| &mut g.canvas) {
            canvas.highlight = picked
                .map(crate::authoring::animation::transition_edge)
                .into_iter()
                .collect();
            canvas.live = live;
        }
        out.push_str("<panel id='animation-content' class='tool-body'>");
        out.push_str(&format!(
            "<panel id='animation-side' class='tool-side' style='width:{SIDE}px'>"
        ));
        self.animation_parameters(&doc, &controller, out);
        self.animation_transitions(&controller, picked, out);
        out.push_str("</panel><panel id='animation-side-edge' class='tool-edge' />");
        out.push_str("<panel id='animation-main' class='tool-main'><panel id='animation-canvas' style='flex-grow:1;flex-basis:0px;background:var(--background);overflow:hidden' label='Animation controller canvas' /></panel></panel>");
        self.animation_status(&controller, out);
        Ok(())
    }

    /// Follow the open controller's running animator during Play, at most every
    /// [`LIVE_EVERY`] and only while the Animation page is in front. Clears the live
    /// state when Play stops.
    pub(crate) fn poll_animation(&mut self, editor: &mut Editor) -> Option<LiveChange> {
        let front = editor.state.playing.is_some()
            && (in_front(&editor.state.layout, "animation")
                || editor
                    .state
                    .floating
                    .iter()
                    .any(|f| in_front(&f.root, "animation")));
        let doc = self.resource_for(editor, "animation").filter(|_| front);
        let Some(doc) = doc else {
            let was = self.animation_live.status.is_some();
            self.animation_live = AnimationLive::default();
            if let Some(shown) = self
                .graphs
                .get_mut("animation")
                .filter(|g| g.canvas.live.is_some())
            {
                shown.canvas.live = None;
            }
            return was.then_some(LiveChange::Page);
        };
        if self
            .animation_live
            .polled
            .is_some_and(|at| at.elapsed() < LIVE_EVERY)
        {
            return None;
        }
        self.animation_live.polled = Some(std::time::Instant::now());
        let revision = editor.host.store().revision();
        let key = Some((revision, doc.clone()));
        if self.animation_live.found != key {
            self.animation_live.found = key;
            self.animation_live.entity = animated_entity(editor, &doc);
        }
        let status = self.animation_live.entity.as_ref().and_then(|entity| {
            let path = format!("{entity}.{}", ara_runtime::animation::STATUS);
            let mut params = json!({ "paths": [path] });
            if let Some(world) = &editor.play_world {
                params["world"] = json!(world);
            }
            let read = editor.action("play.read", params).ok()?;
            Some(read["values"][&path].clone()).filter(|v| v.is_object())
        });
        if status == self.animation_live.status {
            return None;
        }
        // The page shows the playing state's name and rounded parameter values; only a
        // change there rebuilds it. State progress moves the canvas alone.
        let shown = |status: &Option<Value>| {
            status.as_ref().map(|s| {
                let parameters = s["parameters"].as_array().map(|p| {
                    p.iter()
                        .map(crate::authoring::animation::live_value)
                        .collect::<Vec<_>>()
                });
                (s["state"].clone(), s["fade"]["target"].clone(), parameters)
            })
        };
        let page = shown(&status) != shown(&self.animation_live.status);
        self.animation_live.status = status;
        let document = editor.host.store().workspace().doc(&doc)?;
        let (_, controller) = crate::authoring::animation::controller(document).ok()?;
        let live = self
            .animation_live
            .status
            .as_ref()
            .filter(|_| !editor.state.animation_blends)
            .and_then(|status| crate::authoring::animation::live(&controller, status));
        if let Some(shown) = self.graphs.get_mut("animation").filter(|g| g.doc == doc) {
            shown.canvas.live = live;
        }
        Some(if page {
            LiveChange::Page
        } else {
            LiveChange::Canvas
        })
    }

    /// Switch between the state machine and the pose graph, framing the new projection.
    pub(crate) fn animation_view(
        &mut self,
        editor: &mut Editor,
        blends: bool,
    ) -> ara_ui::Result<()> {
        if editor.state.animation_blends == blends {
            return Ok(());
        }
        editor.action("editor.workspace", json!({"blends":blends}))?;
        self.rebuild(editor)?;
        self.edit_graph_layout(editor, "frame")
    }

    /// Parameters with their initial values, edited in place (Unity's Parameters tab).
    fn animation_parameters(&mut self, doc: &str, controller: &ControllerData, out: &mut String) {
        out.push_str(&format!("<panel id='animation-params-header' class='side-header'><text id='animation-params-title' class='heading' text='Parameters' /><text id='animation-params-count' class='caption' text='{}' /><panel id='animation-params-fill' class='fill' /></panel>",controller.parameters.len()));
        if controller.parameters.is_empty() {
            out.push_str("<panel id='animation-params-empty-row' style='flex-shrink:0;padding:8px;padding-top:0px'><text id='animation-params-empty' class='caption' style='white-space:normal' text='No parameters. Add a blend state to create speed, or edit the controller source.' /></panel>");
        }
        let live = self
            .animation_live
            .status
            .as_ref()
            .and_then(|s| s["parameters"].as_array());
        for (n, (name, parameter)) in controller.parameters.iter().enumerate() {
            let row = format!("anim-param-{n}");
            let item = format!("param:{name}");
            let (kind, key, value) = match parameter.kind {
                ParameterKind::Float => ("float", "floatValue", json!(parameter.float_value)),
                ParameterKind::Int => ("int", "intValue", json!(parameter.int_value)),
                ParameterKind::Bool => ("bool", "boolValue", json!(parameter.bool_value)),
                ParameterKind::Trigger => ("trigger", "", Value::Null),
            };
            out.push_str(&format!("<panel id='{row}' class='param-row'><text id='{row}-name' class='fg' text='{}' /><text id='{row}-kind' class='caption' text='{kind}' /><panel id='{row}-fill' class='fill' />",escape(name)));
            let id = format!("{row}-value");
            // Play: the running value replaces the initial-value editor.
            if let Some(value) = live.and_then(|values| values.get(n)) {
                out.push_str(&format!("<text id='{id}-live' class='success' text='{}' description='Live value in Play' /></panel>",escape(&crate::authoring::animation::live_value(value))));
                continue;
            }
            if !key.is_empty() {
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::AnimationField(doc.into(), item.clone(), key.into()),
                );
            }
            match parameter.kind {
                ParameterKind::Bool => out.push_str(&format!(
                    "<toggle id='{id}' checked='{}' label='{} initial value' action='Dispatch' />",
                    parameter.bool_value,
                    escape(name)
                )),
                ParameterKind::Trigger => {
                    out.push_str(&format!("<text id='{id}' class='caption' text='unset' />"))
                }
                _ => {
                    let text = self
                        .drafts
                        .get(&format!("@animation|{doc}|{item}|{key}"))
                        .cloned()
                        .unwrap_or_else(|| panels::display(&value));
                    out.push_str(&format!("<field id='{id}' class='number' style='width:64px' text='{}' label='{} initial value' description='Initial {kind} value; Enter applies' action='Dispatch' />",escape(&text),escape(name)));
                }
            }
            out.push_str("</panel>");
        }
    }

    /// Transitions in priority order, each with its conditions spelled out.
    fn animation_transitions(
        &mut self,
        controller: &ControllerData,
        picked: Option<&str>,
        out: &mut String,
    ) {
        out.push_str(&format!("<panel id='animation-transitions-divider' class='divider' /><panel id='animation-transitions-header' class='side-header'><text id='animation-transitions-title' class='heading' text='Transitions' /><text id='animation-transitions-count' class='caption' text='{}' /><panel id='animation-transitions-fill' class='fill' /></panel><scroll id='animation-transitions' style='flex-grow:1;flex-basis:0px;min-height:0px;gap:0px'>",controller.transitions.len()));
        if controller.transitions.is_empty() {
            out.push_str(&format!("<panel id='animation-transitions-empty-row' style='flex-shrink:0;padding:8px;padding-top:0px'><text id='animation-transitions-empty' class='caption' style='white-space:normal' text='{}' /></panel>",escape("No transitions. Drag from a state's edge to another state.")));
        }
        for (n, transition) in controller.transitions.iter().enumerate() {
            let id = format!("transition-{}", transition.id);
            let selected = picked == Some(transition.id.as_str());
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Protocol(
                    "editor.workspace",
                    json!({"item":format!("transition:{}",transition.id)}),
                ),
            );
            let conditions =
                crate::authoring::animation::conditions(transition, &controller.parameters);
            let (main, muted) = if selected {
                ("on-selection", "on-selection-muted")
            } else {
                ("fg", "caption")
            };
            out.push_str(&format!("<button id='{id}' class='transition-row{}' label='{} to {}' description='Priority {}: {}' action='Dispatch'><panel id='{id}-line' class='row' style='min-height:18px'><text id='{id}-priority' class='{muted}' text='{}' /><text id='{id}-route' class='{main}' text='{} → {}' /></panel><text id='{id}-conditions' class='{muted}' style='padding-left:14px;overflow:hidden' text='{}' /></button>",if selected{" transition-selected"}else{""},escape(&transition.from),escape(&transition.to),n+1,escape(&conditions),n+1,escape(&transition.from),escape(&transition.to),escape(&conditions)));
        }
        out.push_str("</scroll>");
    }

    /// Validity first, then the controller's shape.
    fn animation_status(&mut self, controller: &ControllerData, out: &mut String) {
        out.push_str("<panel id='animation-status' class='tool-status'>");
        match crate::authoring::animation::validate(controller) {
            Ok(()) => out.push_str(&format!(
                "{}<text id='animation-status-state' class='success' text='Valid' />",
                icons::icon_in("animation-status-icon", "circle-check", "success")
            )),
            Err(error) => out.push_str(&format!("{}<text id='animation-status-state' class='destructive' text='Invalid' /><text id='animation-status-error' class='fg' text='{}' />",icons::icon_in("animation-status-icon","circle-x","destructive"),escape(&error.message))),
        }
        if let (Some(entity), Some(status)) =
            (&self.animation_live.entity, &self.animation_live.status)
        {
            let state = status["state"]
                .as_u64()
                .and_then(|i| controller.states.keys().nth(i as usize))
                .map_or("?", String::as_str);
            let time = crate::authoring::animation::seconds(
                u32::try_from(status["time"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX),
            );
            out.push_str(&format!("<panel id='animation-status-live-sep' class='toolbar-sep' />{}<text id='animation-status-live' class='success' text='Playing {} · {time}' description='Live in Play on {}' />",icons::icon_in("animation-status-live-icon","play","success"),escape(state),escape(entity)));
        }
        let count = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
        out.push_str(&format!("<panel id='animation-status-fill' class='fill' /><text id='animation-status-stats' class='caption' text='{} · {} · {} · default {} · {}' description='Fixed step: {} ticks at the {} kHz clip timebase' /></panel>",count(controller.states.len(),"state"),count(controller.transitions.len(),"transition"),count(controller.parameters.len(),"parameter"),escape(if controller.initial.is_empty(){"none"}else{&controller.initial}),rate(controller.fixed_step),controller.fixed_step,crate::authoring::animation::ticks_per_second()/1000));
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
        let Some(shown) = self.graphs.get("animation") else {
            return Ok(());
        };
        let (doc, canvas) = (&shown.doc, &shown.canvas);
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
    /// The selected state, pose node or transition as typed rows: toggles for flags,
    /// numbers with units, the pose operation as a segment, and blend samples as a
    /// plot plus a sample table.
    pub(super) fn animation_inspector(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let Some(doc) = self.resource_for(editor, "animation") else {
            return Ok(());
        };
        let (record, controller) = crate::authoring::animation::controller(
            editor
                .host
                .store()
                .workspace()
                .doc(&doc)
                .ok_or("controller missing")?,
        )?;
        let width = self
            .ui
            .find("animation-inspector")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(300.);
        let label = self.label_px(width);
        out.push_str("<scroll id='animation-inspector' style='flex-grow:1;flex-basis:0px;min-height:0px;padding-bottom:12px'>");
        if self.annotation_inspector(editor, out) {
            out.push_str("</scroll>");
            return Ok(());
        }
        let transition = editor
            .state
            .resource_item
            .as_deref()
            .and_then(|s| s.strip_prefix("transition:"))
            .and_then(|id| {
                controller
                    .transitions
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.id == id)
            });
        let node = self.graphs.get("animation").and_then(|g| {
            self.graph_selection(editor, "animation")
                .iter()
                .next()
                .and_then(|id| g.canvas.graph().nodes.get(id))
                .cloned()
        });
        if let Some((priority, t)) = transition {
            let item = format!("transition:{}", t.id);
            header(
                out,
                "animation-transition",
                "arrow-right",
                &format!("{} → {}", t.from, t.to),
                &format!("Transition · priority {}", priority + 1),
            );
            let step = controller.fixed_step;
            self.time_field(
                out,
                &doc,
                &item,
                "duration",
                "Crossfade",
                Some(t.duration),
                step,
                "switches at once",
                label,
            );
            self.time_field(
                out,
                &doc,
                &item,
                "exitTick",
                "Exit time",
                t.exit_tick,
                step,
                "any time",
                label,
            );
            section(out, "animation-conditions", "Conditions");
            if t.conditions.is_empty() {
                out.push_str("<panel id='animation-condition-none' class='prop-row'><panel id='animation-condition-none-cell' class='prop-label'><text id='animation-condition-none-text' text='None: taken as soon as allowed' /></panel></panel>");
            }
            for (n, condition) in t.conditions.iter().enumerate() {
                let single = Transition {
                    conditions: vec![condition.clone()],
                    ..t.clone()
                };
                let text = crate::authoring::animation::conditions(&single, &controller.parameters);
                out.push_str(&format!("<panel id='animation-condition-{n}' class='prop-row'><panel id='animation-condition-{n}-cell' class='prop-label' style='width:{label}px'><text id='animation-condition-{n}-label' text='{}' /></panel><panel id='animation-condition-{n}-value' class='prop-value'><text id='animation-condition-{n}-text' class='fg' text='{}' /></panel></panel>",if n==0{"When"}else{"and"},escape(&text)));
            }
        } else if let Some(node) = node {
            if let Some((key, value)) = node.payload["state"]
                .as_str()
                .and_then(|id| controller.states.get_key_value(id))
            {
                let item = format!("state:{key}");
                header(
                    out,
                    "animation-state",
                    "person-standing",
                    key,
                    if *key == controller.initial {
                        "Default state"
                    } else {
                        "State"
                    },
                );
                self.time_field(
                    out,
                    &doc,
                    &item,
                    "duration",
                    "Duration",
                    Some(value.duration),
                    controller.fixed_step,
                    "from clip",
                    label,
                );
                self.anim_field(
                    out,
                    &doc,
                    &item,
                    "looping",
                    "Looping",
                    &json!(value.looping),
                    label,
                );
                self.anim_field(
                    out,
                    &doc,
                    &item,
                    "syncGroup",
                    "Sync group",
                    &json!(value.sync_group),
                    label,
                );
                self.anim_field(out, &doc, &item, "node", "Pose", &json!(value.node), label);
            }
            if let Some(key) = node.payload["pose"].as_str()
                && let Some(value) = controller.nodes.get(key)
            {
                let item = format!("pose:{key}");
                if node.payload["state"].is_null() {
                    header(out, "animation-pose", "person-standing", key, "Pose");
                } else {
                    section(out, "animation-pose", &format!("Pose · {key}"));
                }
                self.pose_operation(out, &doc, &record, &controller, key, value.kind);
                if value.kind == NodeKind::Clip {
                    self.anim_field(out, &doc, &item, "clip", "Clip", &json!(value.clip), label);
                } else {
                    self.anim_field(
                        out,
                        &doc,
                        &item,
                        "parameterX",
                        if value.kind == NodeKind::Directional2d {
                            "Horizontal"
                        } else {
                            "Parameter"
                        },
                        &json!(value.parameter_x),
                        label,
                    );
                    if value.kind == NodeKind::Directional2d {
                        self.anim_field(
                            out,
                            &doc,
                            &item,
                            "parameterY",
                            "Vertical",
                            &json!(value.parameter_y),
                            label,
                        );
                    }
                    if !value.samples.is_empty() {
                        blend_plot(out, value, width - 24.);
                        section(out, "blend-samples", "Samples");
                        for (n, sample) in value.samples.iter().enumerate() {
                            let row = format!("blend-sample-{n}");
                            out.push_str(&format!("<panel id='{row}' class='prop-row'><panel id='{row}-label-cell' class='prop-label' style='width:{label}px'><text id='{row}-label' class='fg' text='{}' /></panel><panel id='{row}-value' class='prop-value'>",escape(&sample.node)));
                            for (axis, coordinate) in [("x", sample.x), ("y", sample.y)] {
                                if axis == "y" && value.kind != NodeKind::Directional2d {
                                    continue;
                                }
                                let id = format!("animation-field-sample{n}{axis}");
                                self.bindings.insert(
                                    ara_ui::source::stable_id(&id),
                                    Binding::AnimationField(
                                        doc.clone(),
                                        item.clone(),
                                        format!("sample{n}{axis}"),
                                    ),
                                );
                                let text = self
                                    .drafts
                                    .get(&format!("@animation|{doc}|{item}|sample{n}{axis}"))
                                    .cloned()
                                    .unwrap_or_else(|| panels::display(&json!(coordinate)));
                                out.push_str(&format!("<text id='{id}-axis' class='caption' text='{}' /><field id='{id}' class='number' style='flex-grow:1;flex-basis:0px' text='{}' label='{} {axis}' action='Dispatch' />",axis.to_uppercase(),escape(&text),escape(&sample.node)));
                            }
                            out.push_str("</panel></panel>");
                        }
                    }
                }
            }
        } else {
            self.empty(
                out,
                "animation-select-empty",
                "person-standing",
                "Nothing selected",
                "Select a state or pose on the canvas, or a transition in the list. Drag from a state's edge to another state to add a transition.",
                &[],
            );
        }
        out.push_str("</scroll>");
        Ok(())
    }

    /// The pose operation as a segment; each choice replaces the controller through
    /// the typed controller command.
    fn pose_operation(
        &mut self,
        out: &mut String,
        doc: &str,
        record: &str,
        controller: &ControllerData,
        key: &str,
        current: NodeKind,
    ) {
        // Six choices need the full width: the segment sits on its own row.
        out.push_str("<panel id='animation-operation-row' class='prop-row' style='padding-left:12px'><panel id='animation-operation' class='segment' style='flex-grow:1;flex-basis:0px'>");
        for (kind, short, name) in [
            (NodeKind::Clip, "Clip", "Play one clip"),
            (NodeKind::Blend1d, "1D", "1D blend on one parameter"),
            (NodeKind::Directional2d, "2D", "2D directional blend"),
            (
                NodeKind::Additive,
                "Add",
                "Additive: overlay the second input",
            ),
            (NodeKind::Layer, "Layer", "Layer through a joint mask"),
            (NodeKind::MotionMatching, "Match", "Motion matching"),
        ] {
            let id = format!("animation-operation-{short}");
            let mut changed = controller.clone();
            if let Some(node) = changed.nodes.get_mut(key) {
                node.kind = kind;
            }
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::ResourceContent(
                    doc.into(),
                    vec![json!({"cmd":"editor.animation.controller.set","args":{"record":record,"controller":changed}})],
                    "Change pose operation".into(),
                ),
            );
            out.push_str(&format!("<button id='{id}' class='segment-button{}' style='flex-grow:1;flex-basis:0px' label='{name}' description='{name}' action='Dispatch'><text id='{id}-label' text='{short}' /></button>",if kind==current{" selected"}else{""}));
        }
        out.push_str("</panel></panel>");
    }

    /// A time property, typed and shown in seconds, with frames at the controller's
    /// fixed step beside it and the raw clip ticks in the tooltip. `empty` explains a
    /// zero or missing time. Commits convert seconds back to ticks (`key@s`).
    #[allow(clippy::too_many_arguments)]
    fn time_field(
        &mut self,
        out: &mut String,
        doc: &str,
        item: &str,
        key: &str,
        label: &str,
        ticks: Option<u32>,
        step: u32,
        empty: &str,
        width: f32,
    ) {
        use crate::authoring::animation::{seconds_value, ticks_per_second};
        let id = format!("animation-field-{key}");
        let field = format!("{key}@s");
        self.bindings.insert(
            ara_ui::source::stable_id(&id),
            Binding::AnimationField(doc.into(), item.into(), field.clone()),
        );
        let text = self
            .drafts
            .get(&format!("@animation|{doc}|{item}|{field}"))
            .cloned()
            .unwrap_or_else(|| ticks.map(seconds_value).unwrap_or_default());
        let detail = match ticks {
            Some(t) => format!(
                "{t} ticks at the {} kHz clip timebase. Type seconds (0.1) or milliseconds (100 ms).",
                ticks_per_second() / 1000
            ),
            None => "Type seconds (0.1) or milliseconds (100 ms).".into(),
        };
        let note = match ticks {
            Some(t) if t > 0 && step > 0 => {
                let frames = f64::from(t) / f64::from(step);
                if frames.fract() == 0. {
                    format!("{frames:.0} frames")
                } else {
                    format!("{frames:.1} frames")
                }
            }
            _ => empty.to_owned(),
        };
        out.push_str(&format!("<panel id='{id}-row' class='prop-row'><panel id='{id}-label-cell' class='prop-label' style='width:{width}px'><text id='{id}-label' text='{}' /></panel><panel id='{id}-value' class='prop-value'><field id='{id}' class='number' style='width:64px' text='{}' label='{} in seconds' description='{}' action='Dispatch' /><text id='{id}-unit' class='caption' text='s' /><panel id='{id}-gap' class='fill' /><text id='{id}-note' class='caption' style='padding-right:4px' text='{}' /></panel></panel>",escape(label),escape(&text),escape(label),escape(&detail),escape(&note)));
    }

    /// One typed property row: a toggle for flags, else a field.
    #[allow(clippy::too_many_arguments)]
    fn anim_field(
        &mut self,
        out: &mut String,
        doc: &str,
        item: &str,
        key: &str,
        label: &str,
        value: &Value,
        width: f32,
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
        out.push_str(&format!("<panel id='{id}-row' class='prop-row'><panel id='{id}-label-cell' class='prop-label' style='width:{width}px'><text id='{id}-label' text='{}' /></panel><panel id='{id}-value' class='prop-value'>",escape(label)));
        if let Some(v) = value.as_bool() {
            out.push_str(&format!(
                "<toggle id='{id}' checked='{v}' label='{}' action='Dispatch' />",
                escape(label)
            ));
        } else {
            let text = self
                .drafts
                .get(&format!("@animation|{doc}|{item}|{key}"))
                .cloned()
                .unwrap_or_else(|| {
                    if value.is_null() {
                        String::new()
                    } else {
                        panels::display(value)
                    }
                });
            out.push_str(&format!("<field id='{id}' class='{}' style='flex-grow:1;flex-basis:0px' text='{}' label='{}' action='Dispatch' />",if value.is_number(){"number"}else{""},escape(&text),escape(label)));
        }
        out.push_str("</panel></panel>");
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
        // Times are typed in seconds (`key@s`) and stored as clip ticks.
        let (key, value) = match key.strip_suffix("@s") {
            Some(key) => {
                let ticks = crate::authoring::animation::ticks(&value)?;
                // A blank duration means zero: inferred from the clip, or an instant switch.
                (key, json!(ticks.or((key == "duration").then_some(0))))
            }
            None => (key, value),
        };
        match kind {
            "param" => {
                let parameter = controller
                    .parameters
                    .get_mut(name)
                    .ok_or("parameter missing")?;
                let mut source = ara_json::to_value(&*parameter)?;
                source[key] = value;
                *parameter = serde_json::from_value(source)?;
            }
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

/// An Inspector header row: icon, title and a caption on the right.
fn header(out: &mut String, id: &str, icon: &str, title: &str, caption: &str) {
    out.push_str(&format!("<panel id='{id}-header' class='component-header'>{}<text id='{id}-title' class='component-title' text='{}' /><panel id='{id}-header-fill' class='fill' /><text id='{id}-caption' class='caption' style='padding-right:6px' text='{}' /></panel>",icons::icon(&format!("{id}-header"),icon),escape(title),escape(caption)));
}

/// A quiet caption between property groups.
fn section(out: &mut String, id: &str, title: &str) {
    out.push_str(&format!("<panel id='{id}-section' class='side-header'><text id='{id}-section-title' class='caption' text='{}' /></panel>",escape(title)));
}

/// Blend samples on their axes (Unity's blend-tree diagram): a 1D blend along one
/// line, a 2D blend on crossed axes; each sample is a labelled dot.
fn blend_plot(out: &mut String, node: &Node, width: f32) {
    let width = width.max(120.);
    let two = node.kind == NodeKind::Directional2d;
    let height = if two { width.min(180.) } else { 64. };
    let (low, high) = if two {
        let reach = node
            .samples
            .iter()
            .map(|s| s.x.abs().max(s.y.abs()))
            .fold(1f32, f32::max);
        (-reach, reach)
    } else {
        let xs = node.samples.iter().map(|s| s.x);
        (xs.clone().fold(0f32, f32::min), xs.fold(1f32, f32::max))
    };
    let span = (high - low).max(1e-3);
    let pad = 20.;
    let px = |x: f32| pad + (x - low) / span * (width - pad * 2.);
    let py = |y: f32| {
        if two {
            pad + (high - y) / span * (height - pad * 2.)
        } else {
            height * 0.5 + 8.
        }
    };
    out.push_str(&format!("<panel id='blend-plot-row' style='flex-shrink:0;padding:6px;padding-left:12px;padding-right:12px'><panel id='blend-space-plot' class='blend-plot' style='width:{width}px;height:{height}px'>"));
    out.push_str(&format!(
        "<panel id='blend-axis-x' class='blend-axis' style='left:{pad}px;top:{}px;width:{}px;height:1px' />",
        py(0.),
        width - pad * 2.
    ));
    if two {
        out.push_str(&format!(
            "<panel id='blend-axis-y' class='blend-axis' style='left:{}px;top:{pad}px;width:1px;height:{}px' />",
            px(0.),
            height - pad * 2.
        ));
    }
    for (n, sample) in node.samples.iter().enumerate() {
        let (x, y) = (px(sample.x), py(sample.y));
        out.push_str(&format!("<panel id='blend-point-{n}' class='blend-dot' style='left:{}px;top:{}px' description='{}: ({}, {})' /><text id='blend-point-label-{n}' class='blend-label' style='left:{}px;top:{}px' text='{}' />",x-5.,y-5.,escape(&sample.node),sample.x,sample.y,x-10.,y-22.,escape(&sample.node)));
    }
    out.push_str("</panel></panel>");
}

/// A controller's fixed step as a rate ("60 Hz").
/// Whether `panel` is the active tab of its area in `node`.
fn in_front(node: &ara_ui::docking::DockNode, panel: &str) -> bool {
    match node {
        ara_ui::docking::DockNode::Area { tabs, active, .. } => {
            tabs.get(*active).is_some_and(|t| t == panel)
        }
        ara_ui::docking::DockNode::Split { first, second, .. } => {
            in_front(first, panel) || in_front(second, panel)
        }
    }
}

/// The scene entity Play animates with controller `doc`: the selected entity when its
/// `Animator.controller` points at the document, else the first in address order.
fn animated_entity(editor: &Editor, doc: &str) -> Option<String> {
    let path = editor.host.store().workspace().path_of(doc)?;
    let entities = crate::world::resolve(editor.host.store(), &editor.state.doc).ok()?;
    let uses = |id: &String| {
        entities[id]
            .record
            .get("Animator")
            .and_then(|a| a.get("controller"))
            .and_then(Value::as_str)
            .and_then(|r| ara_schema::grammar::parse_ref(r).ok())
            .is_some_and(|r| r.path == Some(path))
    };
    editor
        .state
        .selection
        .iter()
        .find(|id| entities.contains_key(*id) && uses(id))
        .or_else(|| entities.keys().find(|id| uses(id)))
        .cloned()
}

fn rate(step: u32) -> String {
    if step == 0 {
        return "no fixed step".into();
    }
    let hz = f64::from(crate::authoring::animation::ticks_per_second()) / f64::from(step);
    if hz.fract() == 0. {
        format!("{hz:.0} Hz")
    } else {
        format!("{hz:.1} Hz")
    }
}
