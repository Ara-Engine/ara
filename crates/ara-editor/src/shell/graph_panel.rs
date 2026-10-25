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
    /// The authoring toolbar: document breadcrumb, then the page's mode and tools.
    /// Creating documents lives in the Create menu and each page's empty state.
    pub(super) fn resource_toolbar(
        &mut self,
        editor: &Editor,
        out: &mut String,
        domain: &str,
        doc: Option<&str>,
    ) {
        out.push_str(&format!("<panel id='{domain}-toolbar' class='toolbar'>"));
        let path = doc.and_then(|d| editor.host.store().workspace().path_of(d));
        tool_crumbs(out, domain, path);
        if domain == "material" && doc.is_some() {
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
        if domain != "vfx" && doc.is_some() {
            out.push_str("<panel id='graph-sep' class='toolbar-sep' />");
            self.tool_text(out, "graph-add", "graph.add", "plus", "Add node", "ghost");
            out.push_str("<panel id='graph-tools-sep' class='toolbar-sep' />");
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
            if domain == "material" {
                out.push_str("<panel id='graph-view-sep' class='toolbar-sep' />");
                let shown = !self.node_previews.hidden;
                self.tool_icon(
                    out,
                    "graph-previews",
                    "material.node_previews",
                    "image",
                    editor,
                    shown,
                );
            }
        }
        out.push_str(&format!(
            "<panel id='{domain}-toolbar-fill' class='fill' /></panel>"
        ));
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
        let action = if domain == "material" {
            "New material"
        } else {
            "New controller"
        };
        self.empty(out, &format!("{domain}-empty"), icon, title, "Open one from Assets, or create a new one. Graph and text views edit the same document.", &[(&format!("{domain}-empty-new"), action, command)]);
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
    /// Add node: a search popup. With no query, nodes are grouped by category (the
    /// canvas header accents); typing ranks every node by its name and title. Rows
    /// show the human name and signature; the doc line is the tooltip.
    pub(super) fn node_palette(
        &mut self,
        editor: &Editor,
        out: &mut String,
        doc: &str,
    ) -> ara_ui::Result<()> {
        use crate::graph::style;
        let query = editor
            .state
            .searches
            .get("nodes")
            .map(String::as_str)
            .unwrap_or("");
        out.push_str(&format!("<panel id='node-palette' class='palette' style='left:{}px;top:{}px;width:420px;height:440px'>",(self.ui.size[0]-420.)*0.5,(self.ui.size[1]-440.)*0.5));
        // `.search` grows along a toolbar row; here the row is fixed and the list grows.
        out.push_str("<panel id='node-search-row' class='row' style='height:24px;flex-shrink:0'>");
        self.search(out, "nodes", editor);
        out.push_str("</panel>");
        let registry = ara_material::library::registry().map_err(|d| d.to_string())?;
        out.push_str(
            "<scroll id='node-search-results' style='flex-grow:1;flex-basis:0px;gap:1px'>",
        );
        let mut found = registry
            .search("")
            .filter(|k| k.name != "material.output" && k.name != "material.custom")
            .filter_map(|k| {
                let op = k.name.trim_start_matches("material.");
                let title = style::op_title(op);
                let score = [
                    palette::fuzzy(&k.name, query),
                    palette::fuzzy(&title, query),
                ]
                .into_iter()
                .flatten()
                .map(|(score, _)| score)
                .min()?;
                let category = style::category(op).0;
                let order = style::CATEGORIES
                    .iter()
                    .position(|c| *c == category)
                    .unwrap_or(usize::MAX);
                Some((if query.is_empty() { order } else { score }, title, k))
            })
            .collect::<Vec<_>>();
        found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
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
        let mut group = "";
        for (n, (_, title, kind)) in found.into_iter().take(64).enumerate() {
            let op = kind.name.trim_start_matches("material.");
            let category = style::category(op).0;
            if query.is_empty() && category != group {
                group = category;
                out.push_str(&format!(
                    "<text id='node-group-{n}' class='palette-group' text='{category}' />"
                ));
            }
            let id = format!("node-add-{n}");
            self.bindings.insert(ara_ui::source::stable_id(&id), Binding::ResourceContent(doc.into(),vec![json!({"cmd":"editor.material.node.add","args":{"kind":kind.name,"position":position}})],"Add material node".into()));
            // "lerp(T,T,float) -> T: linear interpolation …" → the signature before the colon.
            let signature = kind
                .doc
                .split_once(": ")
                .map(|(head, _)| head)
                .filter(|head| head.contains('('))
                .unwrap_or("")
                .replace("->", "→");
            out.push_str(&format!("<button id='{id}' class='palette-result node-result' label='{}' description='{}' action='Dispatch'><text id='{id}-title' text='{}' /><panel id='{id}-fill' class='fill' /><text id='{id}-signature' class='palette-id' text='{}' /></button>",escape(&title),escape(&kind.doc),escape(&title),escape(&signature)));
        }
        out.push_str("</scroll></panel>");
        Ok(())
    }
    /// Paint the canvas into the display list right after the canvas panel's own
    /// background. Returns the splice index and how many commands went in there, so
    /// the caller can shift indices it holds past that point.
    pub(super) fn paint_graph(&mut self) -> (usize, usize) {
        let none = (usize::MAX, 0);
        let Some(index) = self.ui.find("graph-canvas") else {
            return none;
        };
        let rect = self.ui.nodes[index].rect;
        let Some(canvas) = &self.canvas else {
            return none;
        };
        let token = |name: &str| {
            self.ui
                .sheet
                .color_token(name)
                .unwrap_or(ara_ui::style::Color(0, 0, 0, 255))
        };
        let background = token("--background");
        // Shadows only read on dark canvases; light themes get a faint lift.
        let dark =
            u16::from(background.0) + u16::from(background.1) + u16::from(background.2) < 384;
        let theme = crate::graph::Theme {
            background,
            card: token("--card"),
            raised: token("--raised"),
            border: token("--border"),
            separator: token("--separator-strong"),
            foreground: token("--foreground"),
            muted: token("--muted-foreground"),
            accent: token("--primary"),
            error: token("--destructive-foreground"),
            success: token("--success-foreground"),
            chart: [1, 2, 3, 4, 5, 6].map(|n| token(&format!("--chart-{n}"))),
            shadow: ara_ui::style::Color(0, 0, 0, if dark { 90 } else { 22 }),
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
        let count = display.commands.len();
        self.ui
            .display
            .commands
            .splice(at..at, display.commands.drain(..));
        (at, count)
    }
}

/// The open document as a breadcrumb ("samples / material.graph"); the folder is
/// muted and the file name reads as the page title.
pub(super) fn tool_crumbs(out: &mut String, domain: &str, path: Option<&str>) {
    out.push_str(&format!("<panel id='{domain}-crumbs' class='tool-crumbs'>"));
    match path {
        Some(path) => {
            let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
            if !folder.is_empty() {
                out.push_str(&format!("<text id='{domain}-crumb-folder' class='crumb' text='{}' /><text id='{domain}-crumb-sep' class='crumb' text='/' />",escape(folder)));
            }
            out.push_str(&format!(
                "<text id='{domain}-document' class='crumb-strong' text='{}' />",
                escape(file)
            ));
        }
        None => out.push_str(&format!(
            "<text id='{domain}-document' class='crumb' text='No document open' />"
        )),
    }
    out.push_str("</panel>");
}
