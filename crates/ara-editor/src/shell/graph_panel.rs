//! One graph widget hosts domain projections; the DSL is another view of the same ops.
use super::*;
impl Shell {
    pub(super) fn material_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let resource = self.resource_for(editor, "material");
        self.resource_toolbar(editor, out, "material", resource.as_deref());
        let Some(doc) = resource else {
            self.resource_empty(out, "material");
            return Ok(());
        };
        self.load_canvas(editor, &doc)?;
        self.apply_canvas_view(editor);
        let graph = self
            .canvas
            .as_ref()
            .ok_or("material canvas missing")?
            .graph();
        let (source, mut diagnostics) = match ara_material::Material::from_graph(graph) {
            Ok(material) => {
                let source = ara_material::dsl::print(&material).map_err(|d| d.to_string())?;
                self.material_sources.insert(doc.clone(), source.clone());
                (
                    source,
                    ara_material::check(&material).err().unwrap_or_default(),
                )
            }
            Err(diagnostics) => (
                self.material_sources.get(&doc).cloned().unwrap_or_else(|| {
                    "// Invalid material nodes: undo or repair their inputs.".into()
                }),
                diagnostics,
            ),
        };
        for diagnostic in &mut diagnostics {
            if diagnostic.location.record.is_none() {
                let invalid = graph.nodes.iter().find(|(_, node)| {
                    let value = node.params.get("value").cloned().unwrap_or(Value::Null);
                    match node.kind.as_str() {
                        "material.parameter" => {
                            serde_json::from_value::<ara_material::ParameterValue>(value).is_err()
                                || node.params.get("isStatic").is_some_and(|v| !v.is_boolean())
                        }
                        "material.literal" => serde_json::from_value::<Vec<f64>>(value).is_err(),
                        _ => false,
                    }
                });
                if let Some((id, _)) = invalid {
                    diagnostic.location.record = Some(id.clone());
                }
            }
        }
        if let Some(canvas) = &mut self.canvas {
            canvas.diagnostics(&diagnostics, &mut self.text);
        }
        out.push_str("<panel id='material-content' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px'><panel id='material-graph-area' style='flex-grow:0.62;flex-basis:0px;overflow:hidden'><panel id='graph-canvas' style='flex-grow:1;flex-basis:0px;background:var(--background);overflow:hidden' label='Material node canvas' />");
        if let Some(diagnostic) = self.canvas.as_ref().and_then(|c| c.diagnostic.as_deref()) {
            out.push_str(&format!("<text id='graph-diagnostic' class='destructive' text='{}' style='height:28px;padding:4px' />",escape(diagnostic)));
        }
        out.push_str("</panel><panel id='material-source-pane' style='flex-grow:0.38;flex-basis:0px;border-width:1px;border-color:var(--border);min-height:0px'><panel id='material-source-header' class='toolbar'><text id='material-source-title' class='field-label' text='Material DSL' /><panel id='material-source-fill' style='flex-grow:1' />");
        out.push_str(&self.bind_button(
            "material-source-apply",
            "Apply  Ctrl Enter",
            Binding::ResourceApply,
            "small quiet",
        ));
        let draft = self
            .resource_text
            .as_ref()
            .filter(|(d, _)| d == &doc)
            .map(|(_, s)| s.as_str())
            .unwrap_or(&source);
        self.bindings.insert(
            ara_ui::source::stable_id("material-source"),
            Binding::ResourceText(doc.clone(), "material".into()),
        );
        out.push_str(&format!("</panel><field id='material-source' style='flex-grow:1;flex-basis:0px;height:100%;font-family:JetBrains Mono;font-size:11px;padding:12px;border-radius:0px;border-width:0px;white-space:normal' text='{}' label='Material DSL source. Ctrl Enter applies changes.' action='Dispatch' />",escape(draft)));
        out.push_str("<panel id='material-preview-footer' class='section' style='height:164px;flex-shrink:0;padding:8px;flex-direction:row;gap:8px'>");
        if let Some(path) = editor.host.store().workspace().path_of(&doc) {
            let source = self
                .canvas
                .as_ref()
                .ok_or("preview graph missing")?
                .graph()
                .write(editor.host.store().schema())
                .map_err(|d| d.to_string())?;
            let key =
                self.thumbnails
                    .request_live(path, source.into_bytes(), editor.state.preview_plane);
            let preview_identity = format!("{doc}:{}", editor.state.preview_plane);
            if self.thumbnail_slots.contains_key(&key) {
                self.material_previews
                    .insert(preview_identity.clone(), key.clone());
            }
            if let Some(error) = self.thumbnail_errors.get(&key) {
                out.push_str(&format!("<text id='material-preview-error' class='destructive' style='white-space:normal;font-size:11px;width:140px' text='{}' />",escape(error)));
            }
            let image_key = if self.thumbnail_slots.contains_key(&key) {
                key
            } else {
                self.material_previews
                    .get(&preview_identity)
                    .cloned()
                    .unwrap_or(key)
            };
            let asset = assets::Asset {
                path: image_key,
                reference: Some(format!("@{path}#{doc}")),
                kind: "graph".into(),
                uses: Vec::new(),
                stamp: 0,
                domain: None,
            };
            self.asset_thumbnail(&asset, out, "material-live-preview", 140.);
        }
        out.push_str("<panel id='material-preview-controls' style='gap:8px;flex-grow:1;flex-basis:0px;min-width:0px'><text id='material-preview-heading' class='heading' text='Live preview' /><text id='material-preview-state' class='badge' style='white-space:normal' text='Checked surface preview' />");
        out.push_str(&self.bind_button(
            "preview-sphere",
            "Sphere",
            Binding::PreviewShape(false),
            "small quiet",
        ));
        out.push_str(&self.bind_button(
            "preview-plane",
            "Plane",
            Binding::PreviewShape(true),
            "small quiet",
        ));
        out.push_str("</panel></panel>");
        for (n, d) in diagnostics.iter().take(3).enumerate() {
            out.push_str(&format!("<text id='material-error-{n}' class='destructive' text='{}' style='white-space:normal;font-size:11px;padding:4px' />",escape(&d.message)));
        }
        out.push_str("</panel></panel>");
        Ok(())
    }
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
        out.push_str(&format!("<panel id='{domain}-toolbar' class='toolbar'><text id='{domain}-document' class='muted' text='{}' /><panel id='{domain}-toolbar-fill' style='flex-grow:1' />",escape(doc.and_then(|d|editor.host.store().workspace().path_of(d)).unwrap_or("No asset open"))));
        if domain != "vfx" {
            out.push_str(&self.bind_button(
                "graph-add",
                "+ Add node",
                Binding::Protocol("editor.canvas", json!({"add":!editor.state.canvas_add})),
                "small",
            ));
            for (id, label, kind) in [
                ("graph-comment", "Comment", "comment"),
                ("graph-group", "Group", "group"),
                ("graph-reroute", "Reroute", "reroute"),
            ] {
                out.push_str(&self.bind_button(
                    id,
                    label,
                    Binding::GraphLayout(kind.into()),
                    "small quiet",
                ));
            }
        }
        out.push_str(&self.bind_button(
            &format!("resource-create-{domain}"),
            "New",
            Binding::ResourceCreate(domain.into()),
            "small quiet",
        ));
        out.push_str("</panel>");
    }
    fn resource_empty(&self, out: &mut String, domain: &str) {
        out.push_str(&format!("<panel id='{domain}-empty' class='body' style='align-items:center;justify-content:center;gap:12px'><text id='{domain}-welcome' style='font-size:18px' text='Create or open a {domain} asset' /><text id='{domain}-hint' class='muted' text='Choose an asset in the browser, or create one with New.' /></panel>"));
    }
    fn load_canvas(&mut self, editor: &Editor, doc: &str) -> ara_ui::Result<()> {
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
