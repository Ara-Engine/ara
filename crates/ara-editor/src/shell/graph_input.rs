//! Shared graph gestures dispatch domain commands at gesture boundaries.
use super::*;
use ara_platform::Event;
impl Shell {
    pub(super) fn apply_canvas_view(&mut self, editor: &Editor) {
        if !self.canvas_dragging
            && let Some(canvas) = &mut self.canvas
        {
            canvas.camera = editor.state.canvas;
            canvas.selection.clone_from(&editor.state.canvas_selection);
        }
    }
    pub(super) fn create_resource(
        &mut self,
        editor: &mut Editor,
        domain: &str,
    ) -> ara_ui::Result<()> {
        let folder = match domain {
            "material" => "materials",
            "vfx" => "effects",
            "animation" => "animations",
            _ => "graphs",
        };
        let extension = if domain == "animation" {
            "animation"
        } else {
            "graph"
        };
        let path = format!(
            "{folder}/{domain}-{}.{}",
            editor.host.store().revision(),
            extension
        );
        let result = editor.action(
            "editor.resource.create",
            json!({"path":path,"domain":domain}),
        )?;
        editor.action(
            "editor.workspace",
            json!({"resource":result["doc"],"asset":""}),
        )?;
        self.resource_text = None;
        Ok(())
    }
    pub(super) fn apply_resource_text(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let Some((doc, source)) = &self.resource_text else {
            return Ok(());
        };
        editor.apply_to(
            doc,
            vec![json!({"cmd":"material.text.apply","args":{"text":source}})],
            "Edit material DSL",
        )?;
        self.resource_text = None;
        Ok(())
    }
    pub(super) fn edit_graph_layout(
        &mut self,
        editor: &mut Editor,
        kind: &str,
    ) -> ara_ui::Result<()> {
        let (Some(canvas), Some(doc)) = (&self.canvas, &self.canvas_doc) else {
            return Ok(());
        };
        let mut layout = canvas.graph().editor.clone();
        let key = format!("editor{}", editor.host.store().revision());
        match kind {
            "comment" => {
                layout.comments.insert(
                    key,
                    ara_graph::Comment {
                        text: "Comment".into(),
                        position: canvas.world(
                            [self.pointer[0], self.pointer[1]],
                            self.graph_rect().unwrap_or_default(),
                        ),
                    },
                );
            }
            "group" => {
                layout.groups.insert(
                    key,
                    ara_graph::Group {
                        label: "Group".into(),
                        nodes: canvas.selection.iter().cloned().collect(),
                    },
                );
            }
            "reroute" => {
                if let Some((id, edge)) = canvas.graph().edges.iter().next() {
                    let a = canvas
                        .graph()
                        .editor
                        .positions
                        .get(&edge.from.node)
                        .copied()
                        .unwrap_or([0., 0.]);
                    let b = canvas
                        .graph()
                        .editor
                        .positions
                        .get(&edge.to.node)
                        .copied()
                        .unwrap_or([256., 128.]);
                    layout
                        .reroutes
                        .entry(id.clone())
                        .or_default()
                        .push([(a[0] + b[0] + 216.) * 0.5, (a[1] + b[1]) * 0.5]);
                }
            }
            _ => {}
        }
        if canvas.graph().header.domain == "animation" {
            editor.action(
                "editor.animation.layout",
                json!({"doc":doc,"editor":layout}),
            )?;
            return Ok(());
        }
        editor.apply_to(
            doc,
            vec![json!({"cmd":"graph.editor.set","args":{"editor":layout}})],
            "Edit graph annotations",
        )?;
        Ok(())
    }
    fn graph_rect(&self) -> Option<Box2> {
        self.ui.find("graph-canvas").map(|i| self.ui.nodes[i].rect)
    }
    /// Screen-space socket for a semantic scripted interaction, using the live canvas transform.
    pub fn graph_port(&self, endpoint: &ara_graph::Endpoint) -> Option<[f32; 2]> {
        self.canvas
            .as_ref()?
            .endpoint_position(endpoint, self.graph_rect()?)
    }
    fn dirty_canvas(&mut self) {
        if let Some(i) = self.ui.find("graph-canvas") {
            self.ui.nodes[i].dirty.paint = true;
        }
    }
    /// Synchronize protocol-driven view commands without rebuilding graph labels.
    pub fn sync_canvas(&mut self, editor: &Editor) {
        if self.canvas_navigation != editor.view_revision && !self.canvas_dragging {
            if let Some(canvas) = &mut self.canvas {
                canvas.camera = editor.state.canvas;
                canvas.selection.clone_from(&editor.state.canvas_selection);
            }
            self.canvas_navigation = editor.view_revision;
            self.dirty_canvas();
        }
    }
    /// Run the native graph interaction route; also used by scripted UI tests.
    pub fn graph_event(&mut self, editor: &mut Editor, event: &Event) -> ara_ui::Result<bool> {
        let Some(rect) = self.graph_rect() else {
            return Ok(false);
        };
        if editor.state.palette || editor.state.canvas_add {
            return Ok(false);
        }
        let inside = rect.contains(self.pointer[0], self.pointer[1]);
        match event {
            Event::MouseMotion { position, .. } if self.canvas_dragging => {
                self.pointer = *position;
                if let Some(canvas) = &mut self.canvas {
                    canvas.motion(*position);
                }
                self.dirty_canvas();
                Ok(true)
            }
            Event::MouseButton {
                button,
                pressed: true,
            } if inside => {
                if let Some(canvas) = &mut self.canvas {
                    canvas.begin(
                        self.pointer,
                        *button,
                        self.modifiers[0] || self.modifiers[1],
                        rect,
                    );
                    self.canvas_dragging = true;
                }
                self.input.blur(&mut self.ui);
                self.dirty_canvas();
                Ok(true)
            }
            Event::MouseButton { pressed: false, .. } if self.canvas_dragging => {
                self.canvas_dragging = false;
                let edits = if let Some(canvas) = &mut self.canvas {
                    canvas.end(self.pointer, rect).unwrap_or_default()
                } else {
                    Vec::new()
                };
                self.submit_graph_edits(editor, edits)?;
                if let Some(canvas) = &self.canvas {
                    editor.action("editor.canvas",json!({"pan":canvas.camera.pan,"zoom":canvas.camera.zoom,"selection":canvas.selection}))?;
                }
                self.rebuild(editor)?;
                Ok(true)
            }
            Event::Wheel(delta) if inside => {
                if let Some(canvas) = &mut self.canvas {
                    canvas.zoom(self.pointer, delta[1], rect);
                    editor.action(
                        "editor.canvas",
                        json!({"pan":canvas.camera.pan,"zoom":canvas.camera.zoom}),
                    )?;
                }
                self.dirty_canvas();
                Ok(true)
            }
            Event::Key {
                scancode: 41,
                pressed: true,
                ..
            } if self.canvas_dragging => {
                self.canvas_dragging = false;
                if let Some(canvas) = &mut self.canvas {
                    canvas.cancel();
                }
                self.dirty_canvas();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
    fn submit_graph_edits(
        &mut self,
        editor: &mut Editor,
        edits: Vec<crate::graph::Edit>,
    ) -> ara_ui::Result<()> {
        let Some(doc) = self.canvas_doc.as_deref() else {
            return Ok(());
        };
        if self
            .canvas
            .as_ref()
            .is_some_and(|c| c.graph().header.domain == "animation")
        {
            let mut layout = self
                .canvas
                .as_ref()
                .ok_or("animation canvas missing")?
                .graph()
                .editor
                .clone();
            let mut changed = false;
            for edit in edits {
                match edit {
                    crate::graph::Edit::Move(nodes) => {
                        for (node, position) in nodes {
                            layout.positions.insert(node, position);
                        }
                        changed = true;
                    }
                    crate::graph::Edit::Layout(next) => {
                        layout = next;
                        changed = true;
                    }
                    crate::graph::Edit::Connect(from, to) => {
                        self.animation_connect(editor, from, to)?
                    }
                }
            }
            if changed {
                editor.action(
                    "editor.animation.layout",
                    json!({"doc":doc,"editor":layout}),
                )?;
            }
            return Ok(());
        }
        let material = self
            .canvas
            .as_ref()
            .is_some_and(|c| c.graph().header.domain == "material");
        let mut commands = Vec::new();
        for edit in edits {
            match edit {
            crate::graph::Edit::Move(nodes)=>for (node,position) in nodes{commands.push(json!({"cmd":"graph.node.move","args":{"node":node,"position":position}}));},
            crate::graph::Edit::Connect(from,to)=>commands.push(if material {json!({"cmd":"material.connect","args":{"source":from.node,"node":to.node,"port":to.port}})}else{json!({"cmd":"graph.connect","args":{"edge":{"from":from,"to":to}}})}),
            crate::graph::Edit::Layout(layout)=>commands.push(json!({"cmd":"graph.editor.set","args":{"editor":layout}})),
        }
        }
        if !commands.is_empty() {
            editor.apply_to(doc, commands, "Edit graph gesture")?;
            self.resource_text = None;
        }
        Ok(())
    }
}
