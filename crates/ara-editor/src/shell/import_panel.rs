//! Importer settings and bidirectional dependency links for the selected source.
//! Settings edits apply the shared `import.configure` command (the same one the CLI
//! and agents use), so importer selection and versions stay engine-owned.
//! Layout: a header (preview beside name, path and importer), then 24 px property
//! rows (label left, typed control right), the actions, and Uses / Used by.
use super::*;
impl Shell {
    pub(super) fn import_panel(&mut self, editor: &Editor, out: &mut String) -> ara_ui::Result<()> {
        let path = editor
            .state
            .asset_path
            .as_deref()
            .ok_or("no selected asset")?;
        let asset = self
            .browser
            .items
            .iter()
            .find(|a| a.path == path)
            .cloned()
            .unwrap_or(assets::Asset {
                path: path.into(),
                reference: None,
                kind: path.rsplit('.').next().unwrap_or("file").into(),
                uses: Vec::new(),
                stamp: 0,
                domain: None,
            });
        let width = self
            .ui
            .find("import-inspector")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(300.);
        // Importer labels are longer than component fields ("Virtual geometry").
        let label = (width * 0.42).clamp(110., 170.);
        out.push_str("<scroll id='import-inspector' style='flex-grow:1;flex-basis:0px;min-height:0px;padding-bottom:12px'>");
        // The preview renders at 2× its 96 px logical size, separately from grid tiles.
        let mut preview_asset = asset.clone();
        if let Some(doc) = editor
            .host
            .store()
            .workspace()
            .find(path)
            .and_then(|id| editor.host.store().workspace().doc(id.as_str()))
            && doc.kind == "graph"
        {
            let source = ara_doc::write(doc, editor.host.store().schema());
            preview_asset.path =
                self.thumbnails
                    .request_live(path, source.into_bytes(), false, 192);
        } else if matches!(
            asset.kind.as_str(),
            "png" | "jpg" | "jpeg" | "glb" | "gltf" | "fbx"
        ) {
            preview_asset.path = self.thumbnails.request_sized(path, 192);
        }
        // Until the larger render lands, the grid tile's image stands in.
        if !self.thumbnail_slots.contains_key(&preview_asset.path)
            && self.thumbnail_slots.contains_key(&asset.path)
        {
            preview_asset.path = asset.path.clone();
        }
        out.push_str("<panel id='import-header' class='row' style='flex-shrink:0;align-items:start;gap:10px;padding:8px'>");
        self.asset_thumbnail(&preview_asset, out, "import-preview", 96.);
        let sidecar = format!("{path}.import.json");
        let doc = editor
            .host
            .store()
            .workspace()
            .docs()
            .find_map(|(path, doc)| (path == sidecar).then_some(doc));
        // Undoing an import leaves the sidecar without its settings record: that reads
        // as "not imported", like a missing sidecar.
        let imported = doc.and_then(|doc| doc.record("i_settings").map(|record| (doc, record)));
        let importer = imported
            .as_ref()
            .and_then(|(_, record)| record.get("importer"))
            .and_then(Value::as_str)
            .unwrap_or("");
        out.push_str(&format!("<panel id='import-copy' style='flex-grow:1;flex-basis:0px;min-width:0px;gap:4px'><text id='import-title' class='heading' style='font-size:13px;overflow:hidden' text='{}' /><text id='import-path' class='caption' text='{}' style='white-space:normal' />",escape(path.rsplit('/').next().unwrap_or(path)),escape(path)));
        if !importer.is_empty() {
            out.push_str(&format!(
                "<text id='import-engine' class='chip-accent' text='{}' />",
                escape(importer)
            ));
        }
        out.push_str("</panel></panel>");
        if let Some((doc, record)) = imported {
            let settings: ara_asset::ImportSettings =
                serde_json::from_value(Value::Object(record.clone()))?;
            out.push_str("<panel id='import-settings-header' class='component-header'><text id='import-settings-title' class='component-title' style='padding-left:8px' text='Import settings' /></panel>");
            let (defaults, schema) = match settings.importer.as_str() {
                "ara.texture" => (
                    ara_json::to_value(&ara_importers::TextureSettings::default())?,
                    "TextureSettings",
                ),
                "ara.audio" => (
                    ara_json::to_value(&ara_importers::audio::Settings::default())?,
                    "AudioImportSettings",
                ),
                _ => (
                    ara_json::to_value(&ara_importers::MeshSettings::default())?,
                    "MeshSettings",
                ),
            };
            let db = editor.host.store().schema();
            let metadata = db.get(schema).and_then(|ty| {
                if let ara_schema::Shape::Struct { fields, .. } = &ty.shape {
                    Some(fields)
                } else {
                    None
                }
            });
            if let Some(fields) = defaults.as_object() {
                for (key, default) in fields {
                    let value = settings.settings.get(key).unwrap_or(default);
                    let id = format!("import-setting-{key}");
                    let desc = metadata.and_then(|fields| fields.iter().find(|f| f.name == *key));
                    let help = desc.map(|f| f.doc.as_str()).unwrap_or("");
                    let name = match key.as_str() {
                        "lodCount" => "LOD count".to_owned(),
                        "lodError" => "LOD error".to_owned(),
                        "flipNormalY" => "Flip normal Y".to_owned(),
                        "maxDimension" => "Max dimension".to_owned(),
                        _ => super::inspector_panel::sentence(key),
                    };
                    out.push_str(&format!("<panel id='{id}-row' class='prop-row'><panel id='{id}-label-cell' class='prop-label' style='width:{label}px'><text id='{id}-label' text='{}' description='{}' /></panel><panel id='{id}-value' class='prop-value'>",escape(&name),escape(help)));
                    self.bindings.insert(
                        ara_ui::source::stable_id(&id),
                        Binding::ImportField(doc.id.to_string(), key.clone()),
                    );
                    if let Some(v) = value.as_bool() {
                        out.push_str(&format!(
                            "<toggle id='{id}' checked='{v}' label='{}' description='{}' action='Dispatch' />",
                            escape(&name),
                            escape(help)
                        ));
                    } else if let Some(crate::inspector::Widget::Enum { options }) =
                        desc.map(|d| crate::inspector::widget(db, d))
                    {
                        self.choice_button(
                            &format!("{id}-options"),
                            &name,
                            choices::Select {
                                destination: choices::Destination::Import {
                                    doc: doc.id.to_string(),
                                    key: key.clone(),
                                },
                                options: options
                                    .iter()
                                    .map(|option| {
                                        (json!(option), super::inspector_panel::sentence(option))
                                    })
                                    .collect(),
                                current: value.clone(),
                            },
                            out,
                        );
                    } else {
                        let draft = self
                            .drafts
                            .get(&format!("@import|{}|{key}", doc.id))
                            .cloned()
                            .unwrap_or_else(|| {
                                // Empty maps and lists read as blank, not raw JSON.
                                match value {
                                    Value::Object(o) if o.is_empty() => String::new(),
                                    Value::Array(a) if a.is_empty() => String::new(),
                                    _ => panels::display(value),
                                }
                            });
                        out.push_str(&format!("<field id='{id}' class='{}' style='flex-grow:1;flex-basis:0px' text='{}' action='Dispatch' label='{}' description='{}' />",if value.is_number(){"number"}else{""},escape(&draft),escape(&name),escape(help)));
                    }
                    out.push_str("</panel></panel>");
                }
            }
            out.push_str(&format!("<panel id='import-quality-row' class='prop-row'><panel id='import-quality-label-cell' class='prop-label' style='width:{label}px'><text id='import-quality-label' text='Quality' description='Final cook quality: Fast iterates, High quality ships' /></panel><panel id='import-quality-value' class='prop-value'><panel id='import-quality' class='segment'>"));
            for (preset, name) in [
                (ara_asset::CookPreset::Fast, "Fast"),
                (ara_asset::CookPreset::Balanced, "Balanced"),
                (ara_asset::CookPreset::HighQuality, "High"),
            ] {
                out.push_str(&self.bind_button(&format!("import-quality-{preset:?}"),name,Binding::ResourceContent(doc.id.to_string(),vec![json!({"cmd":"import.configure","args":{"source":path,"preset":preset}})],"Change import quality".into()),if Some(preset)==settings.preset{"segment-button selected"}else{"segment-button"}));
            }
            out.push_str("</panel></panel></panel><panel id='import-actions' class='row' style='flex-shrink:0;padding:8px;padding-left:12px'>");
            out.push_str(&self.bind_button(
                "asset-reimport",
                "Reimport",
                Binding::Protocol("editor.asset.reimport", json!({"path":path})),
                "",
            ));
            out.push_str("</panel>");
        } else if ara_importers::commands::importer_for_source(path).is_ok() {
            out.push_str("<panel id='import-new' style='flex-shrink:0;gap:8px;padding:8px;padding-left:12px'><text id='import-new-help' class='muted' text='Create settings and cook this source in an isolated worker.' style='white-space:normal' />");
            // Not `asset-import`: element ids (and their binding hashes) stay unique
            // across the separately reconciled parts.
            out.push_str(&self.bind_button(
                "import-source",
                "Import source",
                Binding::Protocol("editor.asset.import", json!({"path":path})),
                "primary",
            ));
            out.push_str("</panel>");
        }
        if matches!(asset.kind.as_str(), "graph" | "animation")
            && let Some(id) = editor.host.store().workspace().find(path)
        {
            out.push_str("<panel id='import-open-row' class='row' style='flex-shrink:0;padding:8px;padding-left:12px'>");
            out.push_str(&self.bind_button(
                "asset-open",
                "Open editor",
                Binding::Protocol("editor.workspace", json!({"resource":id})),
                "primary",
            ));
            out.push_str("</panel>");
        }
        let mut graph = ara_asset::DependencyGraph::default();
        for item in &self.browser.items {
            graph.replace(
                item.path.clone(),
                item.uses.iter().map(|s| {
                    s.trim_start_matches('@')
                        .split('#')
                        .next()
                        .unwrap_or(s)
                        .to_owned()
                }),
            );
        }
        // Live authoring ops take precedence over a background filesystem snapshot.
        for (doc_path, doc) in editor.host.store().workspace().docs() {
            let mut refs = Vec::new();
            for record in doc.records().values() {
                collect_refs(&Value::Object(record.clone()), &mut refs);
            }
            graph.replace(doc_path.to_owned(), refs);
        }
        for (label, links) in [
            (
                "Uses",
                graph
                    .dependencies(path)
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            ),
            (
                "Used by",
                graph.users(path).map(str::to_owned).collect::<Vec<_>>(),
            ),
        ] {
            out.push_str(&format!("<panel id='dep-{label}-header' class='side-header'><text id='dep-{label}-title' class='caption' text='{label}' /><text id='dep-{label}-count' class='caption' text='{}' /></panel>",links.len()));
            if links.is_empty() {
                out.push_str(&format!(
                    "<panel id='dep-{label}-empty-row' style='flex-shrink:0;padding-left:12px;padding-bottom:4px'><text id='dep-{label}-empty' class='caption' text='None' /></panel>"
                ));
            }
            for (n, link) in links.iter().enumerate() {
                out.push_str(&self.bind_button(
                    &format!("dependency-{label}-{n}"),
                    link,
                    Binding::Protocol("editor.workspace", json!({"asset":link})),
                    "small quiet folder-row dep-row",
                ));
            }
        }
        out.push_str("</scroll>");
        Ok(())
    }
    pub(super) fn commit_import_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let keys = self
            .drafts
            .keys()
            .filter(|k| k.starts_with("@import|"))
            .cloned()
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(false);
        }
        // Per sidecar document: its source path and the edited settings object.
        let mut documents = BTreeMap::<String, (String, Value)>::new();
        for key in &keys {
            let mut parts = key.split('|');
            let _ = parts.next();
            let doc = parts.next().ok_or("invalid import draft")?;
            let field = parts.next().ok_or("invalid import field")?;
            if !documents.contains_key(doc) {
                let workspace = editor.host.store().workspace();
                let source = workspace
                    .path_of(doc)
                    .and_then(|p| p.strip_suffix(".import.json"))
                    .ok_or("import document disappeared")?
                    .to_owned();
                let settings = workspace
                    .doc(doc)
                    .and_then(|d| d.record("i_settings"))
                    .and_then(|r| r.get("settings"))
                    .filter(|s| s.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                documents.insert(doc.into(), (source, settings));
            }
            let (_, settings) = documents.get_mut(doc).ok_or("import draft missing")?;
            let text = self.drafts.get(key).ok_or("import draft missing")?;
            settings[field] = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
        }
        for (doc, (source, settings)) in documents {
            editor.apply_to(
                &doc,
                vec![
                    json!({"cmd":"import.configure","args":{"source":source,"settings":settings}}),
                ],
                "Edit importer settings",
            )?;
        }
        for key in keys {
            self.drafts.remove(&key);
        }
        Ok(true)
    }
}
fn collect_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.starts_with('@') => out.push(
            s.trim_start_matches('@')
                .split('#')
                .next()
                .unwrap_or(s)
                .into(),
        ),
        Value::Array(a) => {
            for v in a {
                collect_refs(v, out)
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                collect_refs(v, out)
            }
        }
        _ => {}
    }
}
