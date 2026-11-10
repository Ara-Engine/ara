//! Presets lower to existing typed entity/nested commands, never document writes.
use super::*;
use crate::commands::nested::Operation;

/// Actual reflected component to create at the Scene camera's authoring pivot.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Create {
    Global,
    Box,
    Sphere,
    HeightFog,
    FogVolume,
    Probe,
}
/// Explicit override groups appended in a single normal content transaction.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Preset {
    ManualExposure,
    Interior,
}

fn components(kind: Create) -> (&'static str, Value) {
    match kind {
        Create::Global => (
            "Global post-process volume",
            json!({"PostProcessVolume":{"shape":"global"}}),
        ),
        Create::Box => (
            "Local post-process box",
            json!({"PostProcessVolume":{"shape":"box","blendDistance":2,"priority":10},"Transform":{"scale":[10,5,10]}}),
        ),
        Create::Sphere => (
            "Local post-process sphere",
            json!({"PostProcessVolume":{"shape":"sphere","blendDistance":2,"priority":10},"Transform":{"scale":[8,8,8]}}),
        ),
        Create::HeightFog => ("Height fog", json!({"HeightFog":{}})),
        Create::FogVolume => ("Local fog volume", json!({"LocalFogVolume":{}})),
        Create::Probe => ("Reflection probe", json!({"ReflectionProbe":{}})),
    }
}

fn groups(preset: Preset) -> Vec<Value> {
    let mut groups = vec![json!({"module":"exposure","exposure":"manual","ev100":7})];
    if matches!(preset, Preset::Interior) {
        groups.push(json!({"module":"colorGrading","whiteBalance":4500}));
        groups.push(json!({"module":"bloom","bloom":0.04}));
    }
    groups
}

/// The commands that create `kind` at `at` (one entity), and its name.
pub(crate) fn create_commands(kind: Create, at: Vec3) -> (&'static str, Vec<Value>) {
    let (name, mut components) = components(kind);
    if components.get("Transform").is_none() {
        components["Transform"] = json!({});
    }
    components["Transform"]["position"] = json!([at.x, at.y, at.z]);
    (
        name,
        vec![json!({"cmd":"entity.create","args":{"name":name,"components":components}})],
    )
}

/// The commands that append `preset`'s override groups to every selected post-process
/// volume (appending, so groups an agent wrote survive), and the undo label. Fails, changing
/// nothing, when the selection is empty or not all volumes.
pub(crate) fn preset_commands(
    editor: &Editor,
    resolved: &ara_doc::prefab::Resolved,
    preset: Preset,
) -> ara_ui::Result<(Vec<Value>, &'static str)> {
    let document = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("The lighting scene is unavailable")?;
    if editor.state.selection.is_empty() {
        return Err("Select one or more post-process volumes first".into());
    }
    let groups = groups(preset);
    let mut calls = Vec::new();
    for entity in &editor.state.selection {
        if resolved
            .get(entity)
            .is_none_or(|row| !row.record.contains_key("PostProcessVolume"))
        {
            return Err(
                "Every selected entity must have PostProcessVolume; the preset changed nothing"
                    .into(),
            );
        }
        let selection = vec![entity.clone()];
        let field = crate::inspector::fields(
            editor.host.store().schema(),
            document,
            resolved,
            &selection,
            &BTreeMap::new(),
        )
        .into_iter()
        .find(|field| field.path == "PostProcessVolume.overrides")
        .ok_or("Post-process override schema is unavailable")?;
        for group in &groups {
            calls.extend(collection_adapter::commands(
                editor,
                &selection,
                &field,
                &field.value,
                "",
                Operation::Append,
                group.clone(),
                None,
            )?);
        }
    }
    let why = match preset {
        Preset::ManualExposure => "Append manual exposure override",
        Preset::Interior => "Append interior look overrides",
    };
    Ok((calls, why))
}

impl Shell {
    /// One undoable creation, then explicit human selection; local volumes start with no overrides.
    pub(crate) fn create_lighting(
        &mut self,
        editor: &mut Editor,
        kind: Create,
    ) -> ara_ui::Result<()> {
        let (name, commands) = create_commands(kind, editor.state.camera.target);
        let result = editor.apply(commands, &format!("Create {name}"), None)?;
        if let Some(entity) = result["outputs"][0]["id"].as_str() {
            editor.action("editor.select", json!({"entities":[entity]}))?;
            self.reveal_lighting(editor, entity)?;
        }
        self.open_lighting(editor)
    }

    /// Append instead of replacing a captured array: agent-authored groups and other values survive.
    pub(crate) fn lighting_preset(
        &mut self,
        editor: &mut Editor,
        preset: Preset,
    ) -> ara_ui::Result<()> {
        let resolved = self.resolved(editor)?;
        let (calls, why) = preset_commands(editor, &resolved, preset)?;
        editor.apply(calls, why, None)?;
        self.rebuild(editor)
    }
}
