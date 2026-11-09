//! One guarded typed batch; a failed sidecar validates before any document changes.
use super::*;

pub(super) fn apply(editor: &mut Editor, changes: Vec<Change>, why: &str) -> ara_ui::Result<bool> {
    let revision = editor.host.store().revision();
    let commands = lower(editor, changes)?;
    if commands.is_empty() {
        return Ok(false);
    }
    editor.apply(commands, why, Some(revision))?;
    Ok(true)
}

pub(super) fn lower(editor: &Editor, changes: Vec<Change>) -> ara_ui::Result<Vec<Value>> {
    let workspace = editor.host.store().workspace();
    let mut patches =
        BTreeMap::<String, (String, Value, Option<Option<ara_asset::CookPreset>>)>::new();
    let mut project = serde_json::Map::new();
    let policy = policy(editor)?;
    let stored = serde_json::to_value(&policy.defaults)?;

    for change in changes {
        match &change {
            Change::ProjectPreset(preset) => {
                project.insert("preset".into(), json!(preset));
                continue;
            }
            Change::Reset(importer) => {
                project.insert("resetType".into(), json!(type_key(importer)?));
                continue;
            }
            Change::ProjectField {
                importer,
                field,
                value,
            } => {
                let kind = type_key(importer)?;
                let mut current = project
                    .get(kind)
                    .cloned()
                    .or_else(|| stored.get(kind).filter(|v| !v.is_null()).cloned())
                    .unwrap_or(settings(importer)?.0);
                if current.get(field).is_none() {
                    return Err(
                        "That importer default field is not part of the shared schema".into(),
                    );
                }
                current[field] = value.clone();
                project.insert(kind.into(), current);
                continue;
            }
            _ => {}
        }
        let doc = match &change {
            Change::Setting { doc, .. } | Change::Preset { doc, .. } => doc,
            _ => return Err("Unexpected import policy edit".into()),
        };
        if !patches.contains_key(doc) {
            let document = workspace
                .doc(doc)
                .filter(|doc| doc.kind == "import")
                .ok_or("An import sidecar was removed or changed kind")?;
            let source = workspace
                .path_of(doc)
                .map(|path| workspace.reference_path(path))
                .and_then(|path| path.strip_suffix(".import.json"))
                .ok_or("An import sidecar no longer has a source")?;
            let record = document
                .record("i_settings")
                .ok_or("An import sidecar has no settings record")?;
            let importer = record
                .get("importer")
                .and_then(Value::as_str)
                .ok_or("An import sidecar has no importer identity")?;
            let expected = ara_importers::commands::importer_for_source(source)
                .map_err(|error| error.to_string())?;
            if importer != expected {
                return Err("An import sidecar changed importer; choose its files again".into());
            }
            let current = match record.get("settings") {
                None | Some(Value::Null) => json!({}),
                Some(Value::Object(settings)) => Value::Object(settings.clone()),
                _ => return Err("An import sidecar has malformed settings".into()),
            };
            patches.insert(doc.clone(), (source.into(), current, None));
        }
        let (_, current, preset) = patches.get_mut(doc).ok_or("Import patch disappeared")?;
        match change {
            Change::Setting { field, value, .. } => {
                current[&field] = value;
            }
            Change::Preset { preset: value, .. } => *preset = Some(value),
            _ => return Err("Unexpected import policy edit".into()),
        }
    }
    let mut commands = patches
        .into_values()
        .map(|(source, current, preset)| {
            let mut args = json!({"source":source,"settings":current});
            if let Some(preset) = preset {
                match preset {
                    Some(preset) => args["preset"] = json!(preset),
                    None => args["useProjectPreset"] = json!(true),
                }
            }
            json!({"cmd":"import.configure","args":args})
        })
        .collect::<Vec<_>>();
    if !project.is_empty() {
        commands.push(json!({"cmd":"import.project.configure","args":project}));
    }
    Ok(commands)
}

pub(super) fn policy(editor: &Editor) -> ara_ui::Result<ara_asset::ProjectImportSettings> {
    let workspace = editor.host.store().workspace();
    let doc = workspace
        .docs()
        .find(|(path, _)| *path == ara_asset::PROJECT_IMPORT_SETTINGS_FILE)
        .or_else(|| {
            workspace
                .docs()
                .find(|(path, _)| *path == ara_asset::LEGACY_PROJECT_IMPORT_SETTINGS_FILE)
        })
        .map(|(_, doc)| doc);
    match doc {
        None => Ok(ara_asset::ProjectImportSettings::default()),
        Some(doc) => {
            let record = doc
                .record("pi_settings")
                .ok_or("Project import policy has no pi_settings record")?;
            let policy: ara_asset::ProjectImportSettings = serde_json::from_value(json!(record))?;
            Ok(policy)
        }
    }
}

pub(super) fn type_key(importer: &str) -> ara_ui::Result<&'static str> {
    match importer {
        "ara.texture" | "texture" => Ok("texture"),
        "ara.gltf" | "gltf" => Ok("gltf"),
        "ara.fbx" | "fbx" => Ok("fbx"),
        "ara.audio" | "audio" => Ok("audio"),
        _ => Err("Choose an installed importer type".into()),
    }
}
