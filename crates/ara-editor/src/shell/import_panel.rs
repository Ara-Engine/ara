//! Importer settings and bidirectional dependency links for the selected source.
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
        out.push_str("<scroll id='import-inspector' class='body'>");
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
            preview_asset.path = self
                .thumbnails
                .request_live(path, source.into_bytes(), false);
        }
        self.asset_thumbnail(&preview_asset, out, "import-preview", 128.);
        out.push_str(&format!("<text id='import-title' class='heading' text='{}' /><text id='import-path' class='muted' text='{}' style='white-space:normal' /><panel id='import-divider' class='divider' />",escape(path.rsplit('/').next().unwrap_or(path)),escape(path)));
        let sidecar = format!("{path}.import.json");
        let doc = editor
            .host
            .store()
            .workspace()
            .docs()
            .find_map(|(path, doc)| (path == sidecar).then_some(doc));
        if let Some(doc) = doc {
            let record = doc
                .record("i_settings")
                .ok_or("import settings record missing")?;
            let settings: ara_asset::ImportSettings =
                serde_json::from_value(Value::Object(record.clone()))?;
            out.push_str(&format!("<text id='import-engine' class='badge' text='{}' /><text id='import-settings-title' class='heading' text='Import settings' />",escape(&settings.importer)));
            let defaults = if settings.importer == "ara.texture" {
                ara_json::to_value(&ara_importers::TextureSettings::default())?
            } else {
                ara_json::to_value(&ara_importers::MeshSettings::default())?
            };
            let db = editor.host.store().schema();
            let metadata = db
                .get(if settings.importer == "ara.texture" {
                    "TextureSettings"
                } else {
                    "MeshSettings"
                })
                .and_then(|ty| {
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
                    let label = match key.as_str() {
                        "lodCount" => "LOD count",
                        "lodError" => "LOD error",
                        "flipNormalY" => "Flip normal Y",
                        "maskChannel" => "Mask channel",
                        "maxDimension" => "Maximum dimension",
                        _ => key,
                    };
                    out.push_str(&format!("<panel id='{id}-row' class='field-row'><text id='{id}-label' class='field-label' text='{}' description='{}' />",escape(label),escape(help)));
                    self.bindings.insert(
                        ara_ui::source::stable_id(&id),
                        Binding::ImportField(doc.id.to_string(), key.clone()),
                    );
                    if let Some(v) = value.as_bool() {
                        out.push_str(&format!(
                            "<toggle id='{id}' checked='{v}' action='Dispatch' />"
                        ));
                    } else if let Some(crate::inspector::Widget::Enum { options }) =
                        desc.map(|d| crate::inspector::widget(db, d))
                    {
                        out.push_str(&format!("<panel id='{id}-options' style='gap:4px'>"));
                        for (n, option) in options.iter().enumerate() {
                            out.push_str(&self.bind_button(
                                &format!("{id}-option-{n}"),
                                option,
                                Binding::ImportChoice(
                                    doc.id.to_string(),
                                    key.clone(),
                                    json!(option),
                                ),
                                if value == option {
                                    "small selected"
                                } else {
                                    "small quiet"
                                },
                            ));
                        }
                        out.push_str("</panel>");
                    } else {
                        let draft = self
                            .drafts
                            .get(&format!("@import|{}|{key}", doc.id))
                            .cloned()
                            .unwrap_or_else(|| panels::display(value));
                        out.push_str(&format!("<field id='{id}' class='{}' text='{}' action='Dispatch' label='{}' description='{}' />",if value.is_number(){"number"}else{""},escape(&draft),escape(label),escape(help)));
                    }
                    out.push_str("</panel>");
                }
            }
            out.push_str("<text id='import-quality-label' class='field-label' text='Final quality' /><panel id='import-quality' class='row'>");
            for preset in [
                ara_asset::CookPreset::Fast,
                ara_asset::CookPreset::Balanced,
                ara_asset::CookPreset::HighQuality,
            ] {
                out.push_str(&self.bind_button(&format!("import-quality-{preset:?}"),&format!("{preset:?}"),Binding::ResourceContent(doc.id.to_string(),vec![json!({"cmd":"editor.import.settings","args":{"settings":settings.settings,"preset":preset}})],"Change import quality".into()),if preset==settings.preset{"small selected"}else{"small quiet"}));
            }
            out.push_str("</panel>");
            out.push_str(&self.bind_button(
                "asset-reimport",
                "Reimport",
                Binding::Protocol("editor.asset.reimport", json!({"path":path})),
                "",
            ));
        } else if matches!(
            asset.kind.as_str(),
            "png" | "jpg" | "jpeg" | "hdr" | "exr" | "glb" | "gltf" | "fbx"
        ) {
            out.push_str("<text id='import-new-help' class='muted' text='Create settings and cook this source in an isolated worker.' style='white-space:normal' />");
            out.push_str(&self.bind_button(
                "asset-import",
                "Import source",
                Binding::Protocol("editor.asset.import", json!({"path":path})),
                "",
            ));
        }
        if matches!(asset.kind.as_str(), "graph" | "animation")
            && let Some(id) = editor.host.store().workspace().find(path)
        {
            out.push_str(&self.bind_button(
                "asset-open",
                "Open editor",
                Binding::Protocol("editor.workspace", json!({"resource":id})),
                "",
            ));
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
            out.push_str(&format!("<panel id='dep-{label}-divider' class='divider' /><text id='dep-{label}-title' class='heading' text='{label}' />"));
            if links.is_empty() {
                out.push_str(&format!(
                    "<text id='dep-{label}-empty' class='badge' text='No dependencies' />"
                ));
            }
            for (n, link) in links.iter().enumerate() {
                out.push_str(&self.bind_button(
                    &format!("dependency-{label}-{n}"),
                    link,
                    Binding::Protocol("editor.workspace", json!({"asset":link})),
                    "small quiet folder-row",
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
        let mut documents = BTreeMap::<String, Value>::new();
        for key in &keys {
            let mut parts = key.split('|');
            let _ = parts.next();
            let doc = parts.next().ok_or("invalid import draft")?;
            let field = parts.next().ok_or("invalid import field")?;
            if !documents.contains_key(doc) {
                let settings = editor
                    .host
                    .store()
                    .workspace()
                    .doc(doc)
                    .and_then(|d| d.record("i_settings"))
                    .ok_or("import document disappeared")?;
                let mut value = Value::Object(settings.clone());
                if !value["settings"].is_object() {
                    value["settings"] = json!({});
                }
                documents.insert(doc.into(), value);
            }
            let record = documents.get_mut(doc).ok_or("import draft missing")?;
            let text = self.drafts.get(key).ok_or("import draft missing")?;
            record["settings"][field] = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
        }
        for (doc, record) in documents {
            editor.apply_to(&doc,vec![json!({"cmd":"editor.import.settings","args":{"settings":record["settings"],"preset":record.get("preset").cloned().unwrap_or(json!("highQuality"))}})],"Edit importer settings")?;
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
