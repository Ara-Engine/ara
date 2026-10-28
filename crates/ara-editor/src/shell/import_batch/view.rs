//! Type-filtered multi-edit without taking values from a representative source.
use super::*;

impl Shell {
    pub(in crate::shell) fn multi_import_panel(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<bool> {
        let selected = self.asset_selection.selected();
        let primary = editor.state.asset_path.as_deref();
        if selected.len() < 2
            || primary.is_none_or(|path| !selected.contains(&ara_ui::source::stable_id(path)))
        {
            return Ok(false);
        }
        let workspace = editor.host.store().workspace();
        let mut groups = BTreeMap::<String, Group>::new();
        let mut skipped = 0;
        for asset in &self.browser.items {
            if !selected.contains(&ara_ui::source::stable_id(&asset.path)) {
                continue;
            }
            let Some(source) = asset.authored_path.as_deref() else {
                skipped += 1;
                continue;
            };
            let Some(doc) = crate::asset_tools::paths::import_document(workspace, source) else {
                skipped += 1;
                continue;
            };
            let Some(record) = doc.record("i_settings") else {
                skipped += 1;
                continue;
            };
            let importer = record
                .get("importer")
                .and_then(Value::as_str)
                .ok_or("A selected import has no importer identity")?;
            if settings(importer).is_err() {
                skipped += 1;
                continue;
            }
            groups
                .entry(importer.into())
                .or_insert_with(|| Group {
                    importer: importer.into(),
                    sources: Vec::new(),
                    project: false,
                })
                .sources
                .push(Source {
                    doc: doc.id.to_string(),
                    source: source.into(),
                });
        }
        let mut captured = BTreeMap::new();
        for (_, mut group) in groups {
            group.sources.sort_by(|a, b| a.doc.cmp(&b.doc));
            let signature = group
                .sources
                .iter()
                .map(|source| format!("{}:{}", source.doc, source.source))
                .collect::<Vec<_>>()
                .join("|");
            let token = format!(
                "{}-{:x}",
                group.importer,
                ara_ui::source::stable_id(&signature)
            );
            captured.insert(token, group);
        }
        self.import_batch.groups = captured;
        if self.import_batch.filter.as_ref().is_none_or(|filter| {
            !self
                .import_batch
                .groups
                .values()
                .any(|group| &group.importer == filter)
        }) {
            self.import_batch.filter = self
                .import_batch
                .groups
                .values()
                .next()
                .map(|group| group.importer.clone());
        }
        out.push_str("<scroll id='import-inspector' style='flex-grow:1;flex-basis:0px;min-height:0px;padding:8px;gap:8px'>");
        out.push_str(&format!("<text id='import-batch-title' class='heading' text='{} selected sources' /><text id='import-batch-scope' class='caption' style='white-space:normal' text='Edit one compatible importer type at a time. Changes apply to every source in that type in one undoable transaction. {skipped} selected files have no editable import sidecar.' />",selected.len()));
        if self.import_batch.groups.is_empty() {
            out.push_str("<text id='import-batch-empty' class='caption' style='white-space:normal' text='Import the selected source files first, then edit their shared settings.' /></scroll>");
            return Ok(true);
        }
        let options = self
            .import_batch
            .groups
            .values()
            .map(|group| {
                (
                    json!(group.importer),
                    format!(
                        "{} ({} sources)",
                        label(&group.importer),
                        group.sources.len()
                    ),
                )
            })
            .collect();
        self.choice_button(
            "import-batch-type",
            "Compatible importer",
            choices::Select {
                destination: choices::Destination::ImportGroup,
                current: json!(self.import_batch.filter),
                options,
            },
            out,
        );
        let (token, group) = self
            .import_batch
            .groups
            .iter()
            .find(|(_, group)| Some(&group.importer) == self.import_batch.filter.as_ref())
            .map(|(token, group)| (token.clone(), group.clone()))
            .ok_or("Compatible import group disappeared")?;
        out.push_str(&format!("<text id='import-batch-targets' class='caption' style='white-space:normal' text='Editing {} {} sources; other selected types remain unchanged.' />",group.sources.len(),escape(label(&group.importer))));
        self.import_fields(editor, &token, &group, out)?;
        let presets = group
            .sources
            .iter()
            .map(|source| {
                workspace
                    .doc(&source.doc)
                    .and_then(|doc| doc.record("i_settings"))
                    .and_then(|r| r.get("preset"))
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect::<Vec<_>>();
        let preset = presets
            .first()
            .filter(|first| presets.iter().all(|value| value == *first))
            .cloned()
            .unwrap_or(json!("mixed"));
        self.choice_button(
            "import-batch-preset",
            if preset == "mixed" {
                "Quality (mixed)"
            } else {
                "Cook quality"
            },
            choices::Select {
                destination: choices::Destination::ImportBatch {
                    group: token,
                    key: "$preset".into(),
                },
                current: preset,
                options: vec![
                    (Value::Null, "Use project/default quality".into()),
                    (json!("fast"), "Fast".into()),
                    (json!("balanced"), "Balanced".into()),
                    (json!("highQuality"), "High quality".into()),
                ],
            },
            out,
        );
        out.push_str(&self.bind_button(
            "import-batch-defaults",
            "Project texture quality…",
            Binding::Command(
                "assets.import_defaults",
                super::super::commands::Target::None,
            ),
            "quiet",
        ));
        out.push_str("</scroll>");
        Ok(true)
    }
    pub(super) fn import_fields(
        &mut self,
        editor: &Editor,
        token: &str,
        group: &Group,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let workspace = editor.host.store().workspace();
        let (mut defaults, schema) = settings(&group.importer)?;
        let policy = commands::policy(editor)?;
        let inherited = policy.effective_settings(&group.importer, &json!({}))?;
        if let Some(inherited) = inherited.as_object() {
            for (key, value) in inherited {
                defaults[key] = value.clone();
            }
        }
        let db = editor.host.store().schema();
        let fields = db
            .get(schema)
            .ok_or("Installed importer schema unavailable")?
            .fields();
        let defaults = defaults
            .as_object()
            .ok_or("Importer defaults are not an object")?;
        for (key, default) in defaults {
            if group.importer == "ara.fbx" && key == "opacityMaps" {
                continue;
            }
            let metadata = fields
                .iter()
                .find(|field| field.name == *key)
                .ok_or("Shared importer field is not reflected")?;
            let mut common = group.project.then_some(default);
            let mut mixed = false;
            for source in &group.sources {
                let current = workspace
                    .doc(&source.doc)
                    .and_then(|doc| doc.record("i_settings"))
                    .and_then(|record| record.get("settings"))
                    .and_then(|settings| settings.get(key))
                    .unwrap_or(default);
                match common {
                    None => common = Some(current),
                    Some(old) if old != current => mixed = true,
                    _ => {}
                }
            }
            let value = if mixed {
                Value::Null
            } else {
                common.cloned().unwrap_or_else(|| default.clone())
            };
            let id = format!("import-batch-{key}");
            let name = super::super::inspector_panel::sentence(key);
            let help = if mixed {
                format!(
                    "{} Mixed values across {} sources; editing sets all compatible sources.",
                    metadata.doc,
                    group.sources.len()
                )
            } else {
                metadata.doc.clone()
            };
            out.push_str(&format!("<panel id='{id}-row' class='prop-row'><panel id='{id}-label-cell' class='prop-label' style='width:42%'><text id='{id}-label' text='{}' description='{}' /></panel><panel id='{id}-value' class='prop-value'>",escape(&name),escape(&help)));
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::ImportBatchField(token.to_owned(), key.clone()),
            );
            match crate::inspector::widget(db, metadata) {
                crate::inspector::Widget::Bool => {
                    out.push_str(&format!("<toggle id='{id}' checked='{}' label='{}' description='{}' action='Dispatch' />",value.as_bool().unwrap_or(false),escape(&name),escape(&help)));
                }
                crate::inspector::Widget::Enum { options } => self.choice_button(
                    &format!("{id}-options"),
                    &if mixed {
                        "Mixed values".into()
                    } else {
                        name.clone()
                    },
                    choices::Select {
                        destination: choices::Destination::ImportBatch {
                            group: token.to_owned(),
                            key: key.clone(),
                        },
                        options: enum_values(
                            options,
                            matches!(&metadata.ty, ara_schema::ValueType::Option(_)),
                        ),
                        current: value,
                    },
                    out,
                ),
                widget => {
                    let bounds = match widget {
                        crate::inspector::Widget::Number { min, max, step, .. } => {
                            let mut bounds = format!("class='number' step='{step}'");
                            if let Some(min) = min {
                                bounds.push_str(&format!(" min='{min}'"));
                            }
                            if let Some(max) = max {
                                bounds.push_str(&format!(" max='{max}'"));
                            }
                            bounds
                        }
                        _ => String::new(),
                    };
                    let text = self
                        .drafts
                        .get(&format!("@import-batch|{token}|{key}"))
                        .cloned()
                        .unwrap_or_else(|| {
                            if mixed {
                                String::new()
                            } else {
                                panels::display(&value)
                            }
                        });
                    out.push_str(&format!("<field id='{id}' {bounds} style='flex-grow:1;flex-basis:0px' text='{}' placeholder='{}' label='{}' description='{}' action='Dispatch' />",escape(&text),if mixed{"Mixed values"}else{""},escape(&name),escape(&help)));
                }
            }
            if mixed {
                out.push_str(&format!(
                    "<text id='{id}-mixed' class='caption' text='Mixed' />"
                ));
            }
            out.push_str("</panel></panel>");
        }
        Ok(())
    }
}
pub(super) fn label(importer: &str) -> &str {
    match importer {
        "ara.texture" => "Texture",
        "ara.gltf" => "glTF model",
        "ara.fbx" => "FBX model",
        "ara.audio" => "Audio",
        _ => importer,
    }
}
