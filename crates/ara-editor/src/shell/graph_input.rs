//! Shared graph gestures dispatch domain commands at gesture boundaries. Gestures,
//! graph commands and the navigation state act on the active graph page
//! (`Shell::graph_page`); a press on another graph page makes it active first.
use super::graph_panel::{GRAPH_PAGES, canvas_id};
use super::*;
use ara_platform::Event;
impl Shell {
    /// The active graph page's canvas, mutably.
    fn canvas_mut(&mut self) -> Option<&mut crate::graph::Canvas> {
        self.graphs.get_mut(self.graph_page).map(|g| &mut g.canvas)
    }
    /// The active page's canvas takes the navigation state's view and selection.
    pub(super) fn apply_canvas_view(&mut self, editor: &Editor) {
        if !self.canvas_dragging
            && let Some(canvas) = self.canvas_mut()
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
        let folder = &editor.state.asset_folder;
        let extension = if domain == "animation" {
            "animation"
        } else {
            "graph"
        };
        let path = format!(
            "{}{domain}-{}.{}",
            if folder.is_empty() {
                String::new()
            } else {
                format!("{folder}/")
            },
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
        // "frame" fits the whole graph (F); "open" frames a graph that just opened at
        // a readable zoom.
        if kind == "frame" || kind == "open" {
            if let (Some(shown), Some(rect)) = (self.active_graph(), self.graph_rect()) {
                let canvas = &shown.canvas;
                let camera = if kind == "open" {
                    canvas.framed_readable(rect)
                } else {
                    canvas.framed(rect)
                };
                editor.action(
                    "editor.canvas",
                    json!({"pan":camera.pan,"zoom":camera.zoom}),
                )?;
                self.apply_canvas_view(editor);
                self.dirty_canvas();
            }
            return Ok(());
        }
        let Some(shown) = self.active_graph() else {
            return Ok(());
        };
        let (canvas, doc) = (&shown.canvas, &shown.doc);
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
    /// The active graph page's canvas rectangle, when it is in the current surface.
    fn graph_rect(&self) -> Option<Box2> {
        self.ui
            .find(&canvas_id(self.graph_page))
            .map(|i| self.ui.nodes[i].rect)
    }
    /// Screen-space socket for a semantic scripted interaction, using the live canvas transform.
    pub fn graph_port(&self, endpoint: &ara_graph::Endpoint) -> Option<[f32; 2]> {
        self.active_graph()?
            .canvas
            .endpoint_position(endpoint, self.graph_rect()?)
    }
    fn dirty_canvas(&mut self) {
        if let Some(i) = self.ui.find(&canvas_id(self.graph_page)) {
            self.ui.nodes[i].dirty.paint = true;
        }
    }
    /// Synchronize protocol-driven view commands without rebuilding graph labels.
    pub fn sync_canvas(&mut self, editor: &Editor) {
        if self.canvas_navigation != editor.view_revision && !self.canvas_dragging {
            if let Some(canvas) = self.canvas_mut() {
                canvas.camera = editor.state.canvas;
                canvas.selection.clone_from(&editor.state.canvas_selection);
            }
            self.canvas_navigation = editor.view_revision;
            self.dirty_canvas();
        }
    }
    /// Run the native graph interaction route; also used by scripted UI tests.
    pub fn graph_event(&mut self, editor: &mut Editor, event: &Event) -> ara_ui::Result<bool> {
        if let Event::MouseButton { pressed: true, .. } = event
            && let Some(page) = GRAPH_PAGES.into_iter().find(|page| {
                *page != self.graph_page
                    && self.shown_node(&format!("body-{page}")).is_some_and(|i| {
                        self.ui.nodes[i]
                            .rect
                            .contains(self.pointer[0], self.pointer[1])
                    })
            })
        {
            editor.action("editor.focus", json!({"panel": page}))?;
            self.follow_focus(editor)?;
            self.apply_canvas_view(editor);
        }
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
                if let Some(canvas) = self.canvas_mut() {
                    canvas.motion(*position);
                }
                self.dirty_canvas();
                Ok(true)
            }
            Event::MouseButton {
                button,
                pressed: true,
            } if inside => {
                let (pointer, add) = (self.pointer, self.modifiers[0] || self.modifiers[1]);
                if let Some(canvas) = self.canvas_mut() {
                    canvas.begin(pointer, *button, add, rect);
                    self.canvas_dragging = true;
                }
                self.input.blur(&mut self.ui);
                self.dirty_canvas();
                Ok(true)
            }
            Event::MouseButton { pressed: false, .. } if self.canvas_dragging => {
                self.canvas_dragging = false;
                let pointer = self.pointer;
                let edits = if let Some(canvas) = self.canvas_mut() {
                    canvas.end(pointer, rect).unwrap_or_default()
                } else {
                    Vec::new()
                };
                self.submit_graph_edits(editor, edits)?;
                if let Some(canvas) = self.active_graph().map(|g| &g.canvas) {
                    editor.action("editor.canvas",json!({"pan":canvas.camera.pan,"zoom":canvas.camera.zoom,"selection":canvas.selection}))?;
                }
                self.rebuild(editor)?;
                Ok(true)
            }
            Event::Wheel(delta) if inside => {
                let pointer = self.pointer;
                if let Some(canvas) = self.canvas_mut() {
                    canvas.zoom(pointer, delta[1], rect);
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
                if let Some(canvas) = self.canvas_mut() {
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
        let Some(shown) = self.active_graph() else {
            return Ok(());
        };
        let doc = shown.doc.clone();
        let doc = doc.as_str();
        let domain = shown.canvas.graph().header.domain.clone();
        if domain == "animation" {
            let mut layout = shown.canvas.graph().editor.clone();
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
        let material = domain == "material";
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
