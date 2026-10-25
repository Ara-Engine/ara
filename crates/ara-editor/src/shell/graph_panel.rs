//! Shared authoring-tool plumbing: the graph canvases (one per graph page, loaded
//! per document), the resource toolbar, the node palette and canvas painting. The
//! Material, VFX and Animation tools build on it.
use super::commands::Target;
use super::*;
use std::collections::BTreeSet;

/// Pages with a node-graph canvas. Each keeps its own [`GraphCanvas`], so both can be
/// on screen at once; the active one ([`Shell::follow_focus`]) follows the navigation
/// state (`editor.canvas`) and takes graph commands and keys.
pub(super) const GRAPH_PAGES: [&str; 2] = ["material", "animation"];

/// One graph page's canvas and the document it shows.
pub(super) struct GraphCanvas {
    pub canvas: crate::graph::Canvas,
    pub doc: String,
    /// Store revision the canvas was built at.
    pub revision: u64,
    /// A newly opened document, not yet fitted to its laid-out canvas.
    pub fresh: bool,
}

/// The canvas panel's ID on a graph page (`material-canvas`).
pub(super) fn canvas_id(page: &str) -> String {
    format!("{page}-canvas")
}

/// No selection, for pages without a canvas.
static NO_SELECTION: BTreeSet<String> = BTreeSet::new();

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
            self.tool_icon(out, "material-frame", "graph.frame", "focus", editor, false);
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
    /// Load `doc` into `page`'s canvas unless it already shows it at this revision.
    pub(super) fn load_canvas(
        &mut self,
        editor: &Editor,
        page: &'static str,
        doc: &str,
    ) -> ara_ui::Result<()> {
        let revision = editor.host.store().revision();
        if self
            .graphs
            .get(page)
            .is_some_and(|g| g.doc == doc && g.revision == revision)
        {
            return Ok(());
        }
        let document = editor
            .host
            .store()
            .workspace()
            .doc(doc)
            .ok_or("graph document disappeared")?;
        let graph = ara_graph::Graph::from_document(document).map_err(|d| d.to_string())?;
        self.set_graph(page, doc, graph, revision);
        Ok(())
    }
    /// Show `graph` on `page`: the same document refreshes in place (its view and
    /// selection stay); another starts a fresh canvas.
    pub(super) fn set_graph(
        &mut self,
        page: &'static str,
        doc: &str,
        graph: ara_graph::Graph,
        revision: u64,
    ) {
        match self.graphs.get_mut(page).filter(|g| g.doc == doc) {
            Some(shown) => {
                shown.canvas.refresh(graph, &mut self.text);
                shown.revision = revision;
            }
            None => {
                let canvas = crate::graph::Canvas::new(graph, &mut self.text);
                self.graphs.insert(
                    page,
                    GraphCanvas {
                        canvas,
                        doc: doc.into(),
                        revision,
                        fresh: true,
                    },
                );
            }
        }
    }
    /// The active graph page's canvas and document.
    pub(super) fn active_graph(&self) -> Option<&GraphCanvas> {
        self.graphs.get(self.graph_page)
    }
    /// A graph page's node selection: the navigation state's while the page is
    /// active, its canvas's own otherwise.
    pub(super) fn graph_selection<'a>(
        &'a self,
        editor: &'a Editor,
        page: &str,
    ) -> &'a BTreeSet<String> {
        if page == self.graph_page {
            &editor.state.canvas_selection
        } else {
            self.graphs
                .get(page)
                .map_or(&NO_SELECTION, |g| &g.canvas.selection)
        }
    }
    /// The active graph page follows panel focus (a tab, a press inside the page, an
    /// opened document). On a switch the page's own view and selection become the
    /// navigation state; a page about to show another document starts unframed.
    pub(super) fn follow_focus(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let Some(page) = GRAPH_PAGES.into_iter().find(|p| *p == editor.state.focus) else {
            return Ok(());
        };
        if page == self.graph_page {
            return Ok(());
        }
        self.graph_page = page;
        self.canvas_dragging = false;
        let doc = self.resource_for(editor, page);
        let (camera, selection) = self
            .graphs
            .get(page)
            .filter(|g| Some(&g.doc) == doc.as_ref())
            .map(|g| (g.canvas.camera, g.canvas.selection.clone()))
            .unwrap_or_default();
        editor.action(
            "editor.canvas",
            json!({"pan": camera.pan, "zoom": camera.zoom, "selection": selection}),
        )?;
        Ok(())
    }
    /// Fit newly opened graphs at a readable zoom once their canvas is laid out: the
    /// active page through the navigation state (unless a view was already set),
    /// the others directly. True if any canvas moved.
    pub(super) fn frame_new_graphs(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut framed = false;
        for page in GRAPH_PAGES {
            let Some(node) = self.ui.find(&canvas_id(page)) else {
                continue;
            };
            let rect = self.ui.nodes[node].rect;
            let Some(shown) = self.graphs.get_mut(page).filter(|g| g.fresh) else {
                continue;
            };
            shown.fresh = false;
            if page != self.graph_page {
                shown.canvas.camera = shown.canvas.framed_readable(rect);
                self.ui.nodes[node].dirty.paint = true;
                framed = true;
            } else if editor.state.canvas == crate::graph::Camera::default() {
                self.edit_graph_layout(editor, "open")?;
                framed = true;
            }
        }
        Ok(framed)
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
            .find(&canvas_id("material"))
            .map(|i| self.ui.nodes[i].rect)
            .unwrap_or_default();
        let position = self
            .graphs
            .get("material")
            .map(|g| &g.canvas)
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
    /// Paint every graph page's canvas on screen into the display list, each right
    /// after its canvas panel's own background. Returns each splice (index, commands
    /// inserted) in order, so the caller can shift indices it holds past them.
    pub(super) fn paint_graphs(&mut self) -> Vec<(usize, usize)> {
        GRAPH_PAGES
            .into_iter()
            .filter_map(|page| self.paint_graph(page))
            .collect()
    }
    fn paint_graph(&mut self, page: &str) -> Option<(usize, usize)> {
        let id = canvas_id(page);
        let rect = self.ui.nodes[self.ui.find(&id)?].rect;
        let canvas = &self.graphs.get(page)?.canvas;
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
        let id = ara_ui::source::stable_id(&id);
        let at=self.ui.display.commands.iter().position(|d|matches!(d,ara_ui::paint::Draw::Rect{node,..}|ara_ui::paint::Draw::RoundedRect{node,..} if *node==id)).map(|i|i+1).unwrap_or(self.ui.display.commands.len());
        let count = display.commands.len();
        self.ui
            .display
            .commands
            .splice(at..at, display.commands.drain(..));
        Some((at, count))
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
