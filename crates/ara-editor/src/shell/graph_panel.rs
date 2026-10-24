//! Shared authoring-tool plumbing: the graph canvas (loaded per document), the
//! resource toolbar, the node palette and canvas painting. The Material, VFX and
//! Animation tools build on it.
use super::commands::Target;
use super::*;
impl Shell {
    pub(super) fn resource_for(&self, editor: &Editor, domain: &str) -> Option<String> {
        let matches = |doc: &ara_doc::Document| {
            if domain == "animation" {
                doc.kind == "animation"
            } else {
                doc.kind == "graph"
                    && doc
                        .record("g_meta")
                        .and_then(|r| r.get("domain"))
                        .and_then(Value::as_str)
                        == Some(domain)
            }
        };
        editor
            .state
            .resource
            .as_ref()
            .and_then(|id| editor.host.store().workspace().doc(id))
            .filter(|d| matches(d))
            .or_else(|| {
                editor
                    .host
                    .store()
                    .workspace()
                    .docs()
                    .map(|(_, d)| d)
                    .find(|d| matches(d))
            })
            .map(|d| d.id.to_string())
    }
    pub(super) fn resource_toolbar(
        &mut self,
        editor: &Editor,
        out: &mut String,
        domain: &str,
        doc: Option<&str>,
    ) {
        out.push_str(&format!("<panel id='{domain}-toolbar' class='toolbar'>"));
        if domain != "vfx" {
            self.tool_text(out, "graph-add", "graph.add", "plus", "Add node", "ghost");
            out.push_str("<panel id='graph-sep' class='toolbar-sep' />");
            self.tool_icon(out, "graph-frame", "graph.frame", "focus", editor, false);
            self.tool_icon(
                out,
                "graph-comment",
                "graph.comment",
                "file-plus",
                editor,
                false,
            );
            self.tool_icon(
                out,
                "graph-group",
                "graph.group",
                "layout-grid",
                editor,
                false,
            );
            self.tool_icon(
                out,
                "graph-reroute",
                "graph.reroute",
                "git-compare",
                editor,
                false,
            );
        }
        if domain == "material" {
            out.push_str("<panel id='material-view-sep' class='toolbar-sep' /><panel id='material-view' class='segment'>");
            for (id, command, label, on) in [
                (
                    "material-view-graph",
                    "material.view_graph",
                    "Graph",
                    !self.material_source_view,
                ),
                (
                    "material-view-source",
                    "material.view_source",
                    "Source",
                    self.material_source_view,
                ),
            ] {
                self.bind_command(id, command, Target::None);
                out.push_str(&format!("<button id='{id}' class='segment-button{}' label='{label}' description='Show the material as {}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",if on{" selected"}else{""},if command.ends_with("graph"){"a node graph"}else{"DSL source text"}));
            }
            out.push_str("</panel>");
        }
        out.push_str(&format!("<panel id='{domain}-toolbar-fill' class='fill' /><text id='{domain}-document' class='caption' text='{}' />",escape(doc.and_then(|d|editor.host.store().workspace().path_of(d)).unwrap_or("No asset open"))));
        let command = match domain {
            "material" => "assets.create.material",
            "vfx" => "assets.create.vfx",
            _ => "assets.create.animation",
        };
        self.tool_text(
            out,
            &format!("resource-create-{domain}"),
            command,
            "plus",
            "New",
            "ghost",
        );
        out.push_str("</panel>");
    }
    pub(super) fn resource_empty(&mut self, out: &mut String, domain: &str) {
        let (icon, title, command) = match domain {
            "material" => ("contrast", "No material open", "assets.create.material"),
            _ => (
                "person-standing",
                "No controller open",
                "assets.create.animation",
            ),
        };
        self.empty(out, &format!("{domain}-empty"), icon, title, "Open one from Assets, or create a new one. Graph and text views edit the same document.", &[(&format!("{domain}-empty-new"), "Create", command)]);
    }
    pub(super) fn load_canvas(&mut self, editor: &Editor, doc: &str) -> ara_ui::Result<()> {
        if self.canvas_doc.as_deref() != Some(doc)
            || self.canvas_revision != editor.host.store().revision()
        {
            let document = editor
                .host
                .store()
                .workspace()
                .doc(doc)
                .ok_or("graph document disappeared")?;
            let graph = ara_graph::Graph::from_document(document).map_err(|d| d.to_string())?;
            if let Some(canvas) = self
                .canvas
                .as_mut()
                .filter(|_| self.canvas_doc.as_deref() == Some(doc))
            {
                canvas.refresh(graph, &mut self.text);
            } else {
                self.canvas = Some(crate::graph::Canvas::new(graph, &mut self.text));
            }
            self.canvas_doc = Some(doc.into());
            self.canvas_revision = editor.host.store().revision();
        }
        Ok(())
    }
    pub(super) fn node_palette(
        &mut self,
        editor: &Editor,
        out: &mut String,
        doc: &str,
    ) -> ara_ui::Result<()> {
        let query = editor
            .state
            .searches
            .get("nodes")
            .map(String::as_str)
            .unwrap_or("");
        out.push_str(&format!("<panel id='node-palette' class='palette' style='left:{}px;top:{}px;width:360px;height:400px'>",(self.ui.size[0]-360.)*0.5,(self.ui.size[1]-400.)*0.5));
        self.search(out, "nodes", editor);
        let registry = ara_material::library::registry().map_err(|d| d.to_string())?;
        out.push_str(
            "<scroll id='node-search-results' style='flex-grow:1;flex-basis:0px;gap:2px'>",
        );
        let mut found = registry
            .search("")
            .filter(|k| k.name != "material.output" && k.name != "material.custom")
            .filter_map(|k| palette::fuzzy(&k.name, query).map(|(score, _)| (score, k)))
            .collect::<Vec<_>>();
        found.sort_by_key(|(score, kind)| (*score, &kind.name));
        for (n, (_, kind)) in found.into_iter().take(24).enumerate() {
            let centre = self
                .ui
                .find("graph-canvas")
                .map(|i| self.ui.nodes[i].rect)
                .unwrap_or_default();
            let position = self
                .canvas
                .as_ref()
                .map(|c| {
                    c.world(
                        [centre.x + centre.w * 0.45, centre.y + centre.h * 0.4],
                        centre,
                    )
                })
                .unwrap_or([240., 160.]);
            out.push_str(&self.bind_button(&format!("node-add-{n}"),&kind.name,Binding::ResourceContent(doc.into(),vec![json!({"cmd":"editor.material.node.add","args":{"kind":kind.name,"position":position}})],"Add material node".into()),"palette-result"));
        }
        out.push_str("</scroll></panel>");
        Ok(())
    }
    pub(super) fn paint_graph(&mut self) {
        let Some(index) = self.ui.find("graph-canvas") else {
            return;
        };
        let rect = self.ui.nodes[index].rect;
        let Some(canvas) = &self.canvas else {
            return;
        };
        let token = |name| {
            self.ui
                .sheet
                .color_token(name)
                .unwrap_or(ara_ui::style::Color(0, 0, 0, 255))
        };
        let theme = crate::graph::Theme {
            background: token("--background"),
            card: token("--card"),
            border: token("--border"),
            foreground: token("--foreground"),
            muted: token("--muted-foreground"),
            accent: token("--primary"),
            error: token("--destructive-foreground"),
        };
        self.canvas_display.commands.clear();
        self.canvas_display.glyphs.clear();
        canvas.paint(&mut self.canvas_display, rect, theme);
        let display = &mut self.canvas_display;
        let offset = self.ui.display.glyphs.len();
        for draw in &mut display.commands {
            if let ara_ui::paint::Draw::ScalableGlyphRun { glyphs, .. } = draw {
                glyphs.start += offset;
                glyphs.end += offset;
            }
        }
        self.ui.display.glyphs.append(&mut display.glyphs);
        let id = ara_ui::source::stable_id("graph-canvas");
        let at=self.ui.display.commands.iter().position(|d|matches!(d,ara_ui::paint::Draw::Rect{node,..}|ara_ui::paint::Draw::RoundedRect{node,..} if *node==id)).map(|i|i+1).unwrap_or(self.ui.display.commands.len());
        self.ui
            .display
            .commands
            .splice(at..at, display.commands.drain(..));
    }
}
