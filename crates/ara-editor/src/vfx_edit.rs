//! Typed VFX stack edits shared by the editors: module summaries, reorder/duplicate/
//! toggle/remove of a module, and the default module a new stage entry starts from.
use crate::Editor;
use serde_json::{Value, json};

/// A compact value for inline module summaries.
pub(crate) fn short(v: &Value) -> String {
    match v {
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.);
            if f.fract() == 0. {
                format!("{f:.0}")
            } else {
                format!("{f:.2}").trim_end_matches('0').to_owned()
            }
        }
        Value::Array(a) if a.iter().any(|v| v.is_object() || v.is_array()) => {
            format!("{} keys", a.len())
        }
        Value::Array(a) => a.iter().map(short).collect::<Vec<_>>().join(", "),
        Value::String(s) => s.clone(),
        Value::Bool(b) => {
            if *b {
                "on".into()
            } else {
                "off".into()
            }
        }
        _ => "…".into(),
    }
}

/// Moves `up` or `down`, `duplicate`s, `toggle`s or removes the module `item`
/// (`emitter|stage|name`) of the effect `doc` (one undo step).
pub(crate) fn module_edit(
    editor: &mut Editor,
    doc: &str,
    item: &str,
    operation: &str,
) -> ara_ui::Result<()> {
    let doc = doc.to_owned();
    let parts = item.split('|').collect::<Vec<_>>();
    let [emitter_name, stage, name] = parts.as_slice() else {
        return Err("invalid module address".into());
    };
    let graph = ara_graph::Graph::from_document(
        editor
            .host
            .store()
            .workspace()
            .doc(&doc)
            .ok_or("effect missing")?,
    )
    .map_err(|d| d.to_string())?;
    let effect = ara_vfx::Effect::from_graph(&graph).map_err(|d| d.to_string())?;
    let emitter = effect
        .emitters
        .iter()
        .find(|e| e.name == *emitter_name)
        .ok_or("emitter missing")?;
    let stage_value: ara_vfx::Stage = serde_json::from_value(json!(stage))?;
    let modules = match stage_value {
        ara_vfx::Stage::Spawn => &emitter.stacks.spawn,
        ara_vfx::Stage::Initialize => &emitter.stacks.initialize,
        ara_vfx::Stage::Update => &emitter.stacks.update,
        ara_vfx::Stage::Render => &emitter.stacks.render,
    };
    let index = modules
        .iter()
        .position(|m| m.name == *name)
        .ok_or("module missing")?;
    let args = match operation {
        "up" => {
            json!({"emitter":emitter_name,"stage":stage,"name":name,"operation":"move","index":index.saturating_sub(1)})
        }
        "down" => {
            json!({"emitter":emitter_name,"stage":stage,"name":name,"operation":"move","index":(index + 1).min(modules.len() - 1)})
        }
        "duplicate" => {
            let mut copy = modules[index].clone();
            copy.name = format!("{}_copy{}", copy.name, editor.host.store().revision());
            json!({"emitter":emitter_name,"stage":stage,"name":copy.name,"operation":"insert","index":index + 1,"module":copy})
        }
        "toggle" => {
            let enabled = !modules[index].enabled;
            editor.apply_to(
                &doc,
                vec![json!({"cmd":"vfx.module.enable","args":{"emitter":emitter_name,"stage":stage,"name":name,"enabled":enabled}})],
                if enabled { "Enable VFX module" } else { "Disable VFX module" },
            )?;
            return Ok(());
        }
        _ => json!({"emitter":emitter_name,"stage":stage,"name":name,"operation":"remove"}),
    };
    editor.apply_to(
        &doc,
        vec![json!({"cmd":"vfx.stack.edit","args":args})],
        &format!("{} VFX module", crate::names::sentence(operation)),
    )?;
    Ok(())
}
pub(crate) fn default_module(stage: ara_vfx::Stage, revision: u64) -> ara_vfx::StackModule {
    let module = match stage {
        ara_vfx::Stage::Spawn => ara_vfx::Module::Rate { per_second: 20. },
        ara_vfx::Stage::Initialize => ara_vfx::Module::Sphere {
            center: [0., 0., 0.],
            radius: 0.2,
        },
        ara_vfx::Stage::Update => ara_vfx::Module::Drag { coefficient: 0.1 },
        ara_vfx::Stage::Render => ara_vfx::Module::Appearance {
            texture: ara_vfx::SpriteTexture::Fire,
            blend: ara_vfx::Blend::Additive,
            alignment: ara_vfx::Alignment::Camera,
            stretch: 1.,
            shutter: 0.,
        },
    };
    ara_vfx::StackModule::new(format!("module{revision}"), module)
}
