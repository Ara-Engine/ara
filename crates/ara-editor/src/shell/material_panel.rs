//! The Material editor (UE material editor / Shader Graph layout): the live preview
//! and the material's parameters on the left, the node graph (or its DSL source, the
//! same document) in the middle, the selected node in the Inspector, and the compile
//! status along the bottom. Every edit is a typed command on the graph document.
use super::*;

/// Width of the preview + parameters column, in logical pixels.
const SIDE: f32 = 264.;

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
            .graph()
            .clone();
        let material = ara_material::Material::from_graph(&graph).ok();
        let (source, mut diagnostics, used) = match &material {
            Some(material) => {
                let source = ara_material::dsl::print(material).map_err(|d| d.to_string())?;
                self.material_sources.insert(doc.clone(), source.clone());
                match ara_material::check(material) {
                    Ok(checked) => (source, Vec::new(), Some(checked.reachable.len())),
                    Err(diagnostics) => (source, diagnostics, None),
                }
            }
            None => (
                self.material_sources.get(&doc).cloned().unwrap_or_else(|| {
                    "// Invalid material nodes: undo or repair their inputs.".into()
                }),
                ara_material::Material::from_graph(&graph)
                    .err()
                    .unwrap_or_default(),
                None,
            ),
        };
        // Errors without a node point at the first node whose stored value is invalid.
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
        out.push_str("<panel id='material-content' class='tool-body'>");
        out.push_str(&format!(
            "<panel id='material-side' class='tool-side' style='width:{SIDE}px'>"
        ));
        // The page preview is requested before the in-node previews: it renders first.
        self.material_preview(editor, &doc, &graph, out)?;
        self.node_previews(editor, &doc, material.as_ref());
        self.material_parameters(editor, &doc, material.as_ref(), out)?;
        out.push_str("</panel><panel id='material-side-edge' class='tool-edge' />");
        out.push_str("<panel id='material-main' class='tool-main'>");
        if self.material_source_view {
            self.material_source(&doc, &source, out);
        } else {
            out.push_str("<panel id='graph-canvas' style='flex-grow:1;flex-basis:0px;background:var(--background);overflow:hidden' label='Material node canvas' />");
        }
        out.push_str("</panel></panel>");
        self.material_status(material.as_ref(), &diagnostics, used, out);
        Ok(())
    }

    /// In-node previews: derived when the graph changes (at most `node_preview::MAX`),
    /// queued on their own worker and assigned as renders land; until then each node
    /// keeps its last good image. Dropped requests are retried on the next rebuild.
    fn node_previews(
        &mut self,
        editor: &Editor,
        doc: &str,
        material: Option<&ara_material::Material>,
    ) {
        use crate::asset_tools::{node_preview, thumbnails};
        let Some(canvas) = self.canvas.as_mut() else {
            return;
        };
        canvas.show_previews(!self.node_previews.hidden, &mut self.text);
        let pixels = (2. * crate::graph::PREVIEW) as u32;
        let identity = (doc.to_owned(), editor.host.store().revision());
        if self.node_previews.revision.as_ref() != Some(&identity) {
            self.node_previews.revision = Some(identity);
            self.node_previews.requests.clear();
            if let Some(material) = material {
                for node in canvas.preview_nodes().take(node_preview::MAX) {
                    if let Some((derived, shot)) = node_preview::derive(material, node) {
                        let key = thumbnails::Thumbnails::material_key(&derived, shot, pixels);
                        self.node_previews
                            .requests
                            .insert(node.to_owned(), (key, derived, shot));
                    }
                }
            }
        }
        // Tiles only for nodes with a preview rendered or on its way.
        let previewed = self
            .node_previews
            .requests
            .iter()
            .filter(|(_, (key, ..))| !self.node_previews.failed.contains(key))
            .map(|(node, _)| node.clone())
            .collect();
        canvas.set_previewed(previewed, &mut self.text);
        if self.node_previews.hidden {
            return;
        }
        let path = editor
            .host
            .store()
            .workspace()
            .path_of(doc)
            .unwrap_or("material");
        for (node, (key, derived, shot)) in &self.node_previews.requests {
            if self.node_previews.failed.contains(key) {
                continue;
            }
            match self.thumbnail_slots.get(key) {
                Some(slot) => {
                    canvas
                        .previews
                        .insert(node.clone(), thumbnails::image_id(*slot));
                }
                None => self
                    .thumbnails
                    .request_material(path, key, derived, *shot, pixels),
            }
        }
    }

    /// Whether every derived in-node preview has rendered (capture routes).
    pub(crate) fn node_previews_ready(&self) -> bool {
        !self.node_previews.requests.is_empty()
            && self.node_previews.requests.values().all(|(key, ..)| {
                self.thumbnail_slots.contains_key(key) || self.node_previews.failed.contains(key)
            })
    }

    /// The live preview (a checked render of the current graph) and its shape.
    fn material_preview(
        &mut self,
        editor: &Editor,
        doc: &str,
        graph: &ara_graph::Graph,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        out.push_str("<panel id='material-preview' class='tool-preview'>");
        if let Some(path) = editor.host.store().workspace().path_of(doc) {
            let source = graph
                .write(editor.host.store().schema())
                .map_err(|d| d.to_string())?;
            // Rendered at 2× the logical preview size: crisp at 1× and 2× displays.
            let key = self.thumbnails.request_live(
                path,
                source.into_bytes(),
                editor.state.preview_plane,
                (2. * (SIDE - 16.)) as u32,
            );
            self.material_preview_key = Some(key.clone());
            let identity = format!("{doc}:{}", editor.state.preview_plane);
            if self.thumbnail_slots.contains_key(&key) {
                self.material_previews.insert(identity.clone(), key.clone());
            }
            // Until the new render lands, the last good one stays on screen.
            let image = if self.thumbnail_slots.contains_key(&key) {
                key
            } else {
                self.material_previews
                    .get(&identity)
                    .cloned()
                    .unwrap_or(key)
            };
            let asset = assets::Asset {
                path: image,
                reference: Some(format!("@{path}#{doc}")),
                kind: "graph".into(),
                uses: Vec::new(),
                stamp: 0,
                domain: None,
            };
            self.asset_thumbnail(&asset, out, "material-live-preview", SIDE - 16.);
        }
        out.push_str("<panel id='material-preview-bar' class='row'><panel id='material-shape' class='segment'>");
        for (id, label, plane) in [
            ("preview-sphere", "Sphere", false),
            ("preview-plane", "Plane", true),
        ] {
            out.push_str(&self.bind_button(
                id,
                label,
                Binding::PreviewShape(plane),
                if editor.state.preview_plane == plane {
                    "segment-button selected"
                } else {
                    "segment-button"
                },
            ));
        }
        let (state, class) = if self.preview_ready("material") {
            ("Live", "caption")
        } else {
            ("Rendering…", "caption")
        };
        out.push_str(&format!(
            "</panel><panel id='material-preview-fill' class='fill' /><text id='material-preview-state' class='{class}' text='{state}' /></panel></panel><panel id='material-side-divider' class='divider' />"
        ));
        Ok(())
    }

    /// The material's parameters in declaration order, each with its value editor;
    /// clicking a name selects its node (and the Inspector shows it).
    fn material_parameters(
        &mut self,
        editor: &Editor,
        doc: &str,
        material: Option<&ara_material::Material>,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let add = "material-param-add";
        let centre = self
            .canvas
            .as_ref()
            .zip(self.ui.find("graph-canvas").map(|i| self.ui.nodes[i].rect))
            .map(|(canvas, r)| canvas.world([r.x + r.w * 0.3, r.y + r.h * 0.3], r))
            .unwrap_or([120., 120.]);
        self.bindings.insert(
            ara_ui::source::stable_id(add),
            Binding::ResourceContent(
                doc.into(),
                vec![json!({"cmd":"editor.material.node.add","args":{"kind":"material.parameter","position":centre}})],
                "Add material parameter".into(),
            ),
        );
        let count = material.map_or(0, |m| m.parameters.len());
        out.push_str(&format!("<panel id='material-params-header' class='side-header'><text id='material-params-title' class='heading' text='Parameters' /><text id='material-params-count' class='caption' text='{count}' /><panel id='material-params-fill' class='fill' /><button id='{add}' class='ghost icon-button' label='Add parameter' description='Add a float parameter (rename and retype it in the graph or Source)' action='Dispatch'>{}</button></panel>",icons::icon(add,"plus")));
        out.push_str("<scroll id='material-params' style='flex-grow:1;flex-basis:0px;min-height:0px;gap:0px'>");
        self.value_swatches.clear();
        let mut parameters: Vec<_> = material
            .map(|m| m.parameters.iter().collect())
            .unwrap_or_default();
        parameters.sort_by_key(|(_, p)| p.order);
        if parameters.is_empty() {
            out.push_str("<text id='material-params-empty' class='caption' style='padding:8px;white-space:normal' text='No parameters. Parameters are the values material instances and scripts can change.' />");
        }
        for (n, (id, parameter)) in parameters.into_iter().enumerate() {
            let row = format!("mparam-{n}");
            let selected = editor.state.canvas_selection.iter().any(|s| s == id);
            let ty = parameter.ty.to_string();
            self.bindings.insert(
                ara_ui::source::stable_id(&format!("{row}-pick")),
                Binding::Protocol("editor.canvas", json!({"selection":[id]})),
            );
            // The type reads as its pin colour (painted after layout) and in the tooltip.
            self.value_swatches.push((
                format!("{row}-dot"),
                crate::graph::port_color(parameter.ty.name()),
            ));
            out.push_str(&format!("<panel id='{row}' class='param-row{}'><panel id='{row}-dot' class='type-dot' /><button id='{row}-pick' class='param-name' label='{}' description='{} · {} parameter. Click to select its node.' action='Dispatch'><text id='{row}-name' class='{}' text='{}' /></button>",if selected{" param-selected"}else{""},escape(&parameter.name),escape(&parameter.name),escape(&super::inspector_panel::sentence(&ty).to_lowercase()),if selected{"on-selection"}else{"fg"},escape(&parameter.name)));
            let value = format!("{row}-value");
            self.bindings.insert(
                ara_ui::source::stable_id(&value),
                Binding::MaterialValue(doc.into(), id.clone()),
            );
            let draft = self.drafts.get(&format!("@material|{doc}|{id}")).cloned();
            // Texture names are long: the whole name gets its own line under the row.
            let mut below = None;
            match &parameter.default {
                ara_material::ParameterValue::Bool(on) => out.push_str(&format!(
                    "<toggle id='{value}' checked='{on}' label='{}' action='Dispatch' />",
                    escape(&parameter.name)
                )),
                ara_material::ParameterValue::Numeric(lanes) => {
                    if matches!(
                        parameter.ty,
                        ara_material::Type::ColorLinear | ara_material::Type::ColorSrgb
                    ) {
                        // Painted after layout (editor markup carries no raw colours).
                        out.push_str(&format!("<panel id='{row}-swatch' class='swatch' />"));
                        self.value_swatches.push((
                            format!("{row}-swatch"),
                            swatch(lanes, parameter.ty == ara_material::Type::ColorLinear),
                        ));
                    }
                    let text = draft.unwrap_or_else(|| lanes_text(lanes));
                    out.push_str(&format!("<field id='{value}' class='number param-value' text='{}' label='{}' description='{} value; Enter applies' action='Dispatch' />",escape(&text),escape(&parameter.name),escape(&ty)));
                }
                ara_material::ParameterValue::Texture(texture) => {
                    let text = match texture {
                        ara_material::TextureSource::Asset(asset) => asset
                            .0
                            .trim_start_matches('@')
                            .split('#')
                            .next()
                            .and_then(|p| p.rsplit('/').next())
                            .unwrap_or("texture")
                            .to_owned(),
                        ara_material::TextureSource::Default(default) => {
                            format!("{default:?}")
                        }
                    };
                    below = Some(text);
                }
            }
            out.push_str("</panel>");
            if let Some(text) = below {
                out.push_str(&format!(
                    "<panel id='{value}-texture-line' class='param-texture'><text id='{value}-texture' class='caption wrap-text' text='{}' description='{}' /></panel>",
                    escape(&text),
                    escape(&text)
                ));
            }
        }
        out.push_str("</scroll>");
        Ok(())
    }

    /// The DSL view of the same document; Ctrl+Enter (or Apply) commits it.
    fn material_source(&mut self, doc: &str, source: &str, out: &mut String) {
        out.push_str("<panel id='material-source-header' class='toolbar'><text id='material-source-title' class='caption' text='Material DSL · the graph and this text edit the same document' style='padding-left:4px' /><panel id='material-source-fill' class='fill' />");
        out.push_str(&self.bind_button(
            "material-source-apply",
            "Apply  Ctrl+Enter",
            Binding::ResourceApply,
            "primary",
        ));
        let draft = self
            .resource_text
            .as_ref()
            .filter(|(d, _)| d == doc)
            .map(|(_, s)| s.as_str())
            .unwrap_or(source)
            .to_owned();
        self.bindings.insert(
            ara_ui::source::stable_id("material-source"),
            Binding::ResourceText(doc.into(), "material".into()),
        );
        out.push_str(&format!("</panel><field id='material-source' style='flex-grow:1;flex-basis:0px;height:100%;font-family:JetBrains Mono;font-size:12px;padding:12px;border-radius:0px;border-width:0px;white-space:normal' text='{}' label='Material DSL source. Ctrl Enter applies changes.' action='Dispatch' />",escape(&draft)));
    }

    /// Compile status: errors first (the first one selects its node), then the
    /// preview's render state, then what the material costs.
    fn material_status(
        &mut self,
        material: Option<&ara_material::Material>,
        diagnostics: &[ara_diag::Diagnostic],
        used: Option<usize>,
        out: &mut String,
    ) {
        out.push_str("<panel id='material-status' class='tool-status'>");
        let preview_error = self
            .material_preview_key
            .as_ref()
            .and_then(|key| self.thumbnail_errors.get(key))
            .cloned();
        if let Some(first) = diagnostics.first() {
            let count = diagnostics.len();
            out.push_str(&format!(
                "{}<text id='material-status-state' class='destructive' text='{count} error{}' />",
                icons::icon_in("material-status-icon", "circle-x", "destructive"),
                if count == 1 { "" } else { "s" }
            ));
            let message = format!("{} · {}", first.code, first.message);
            if let Some(record) = &first.location.record {
                self.bindings.insert(
                    ara_ui::source::stable_id("material-status-error"),
                    Binding::Protocol("editor.canvas", json!({"selection":[record]})),
                );
                out.push_str(&format!("<button id='material-status-error' class='ghost status-link' label='Select the node' description='Select the node this error is about' action='Dispatch'><text id='material-status-error-text' class='fg' text='{}' /></button>",escape(&message)));
            } else {
                out.push_str(&format!(
                    "<text id='material-status-error-text' class='fg' text='{}' />",
                    escape(&message)
                ));
            }
        } else if let Some(error) = preview_error {
            out.push_str(&format!("{}<text id='material-status-state' class='warning' text='Preview failed' /><text id='material-status-error-text' class='fg' text='{}' />",icons::icon_in("material-status-icon","triangle-alert","warning"),escape(&error)));
        } else if !self.preview_ready("material") {
            out.push_str(&format!(
                "{}<text id='material-status-state' class='muted' text='Compiling preview…' />",
                icons::icon_in("material-status-icon", "refresh-cw", "muted")
            ));
        } else {
            out.push_str(&format!(
                "{}<text id='material-status-state' class='success' text='Compiled' />",
                icons::icon_in("material-status-icon", "circle-check", "success")
            ));
        }
        out.push_str("<panel id='material-status-fill' class='fill' />");
        if let Some(material) = material {
            let nodes = material.parameters.len() + material.bindings.len();
            let used =
                used.map_or_else(String::new, |used| format!(" · {used}/{nodes} nodes used"));
            out.push_str(&format!(
                "<text id='material-status-stats' class='caption' text='{} parameters{used} · {:?} · variant budget {}' />",
                material.parameters.len(),
                material.domain,
                material.variant_budget
            ));
        }
        out.push_str("</panel>");
    }
}

/// Numeric lanes as editable text: `0.35` or `0.18, 0.42, 0.9`.
fn lanes_text(lanes: &[f64]) -> String {
    lanes
        .iter()
        .map(|v| panels::display(&json!(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A colour parameter's swatch: linear lanes are encoded to sRGB for display.
fn swatch(lanes: &[f64], linear: bool) -> ara_ui::style::Color {
    let channel = |i: usize| {
        let v = lanes.get(i).copied().unwrap_or(0.).clamp(0., 1.);
        let v = if linear { v.powf(1. / 2.2) } else { v };
        (v * 255.).round() as u8
    };
    ara_ui::style::Color(channel(0), channel(1), channel(2), 255)
}
