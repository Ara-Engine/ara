//! Fresh layout-v4 fixture and explicitly attributed setup; never edit the supplied project.
use super::super::Options;
use super::*;
pub(in crate::app) fn prepare(options: &mut Options) -> ara_ui::Result<()> {
    let output = std::path::absolute(&options.output)?;
    std::fs::create_dir_all(&output)?;
    let project = output.join(format!("profiler-project-{}", std::process::id()));
    std::fs::create_dir(&project)?;
    let levels = project.join(ara_doc::AUTHORING_ROOT).join("levels");
    std::fs::create_dir_all(&levels)?;
    std::fs::write(
        project.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name":"Native Profiler proof","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"levels/main.scene"}),
        ),
    )?;
    std::fs::write(
        levels.join("main.scene"),
        ara_json::to_canonical_string(
            &json!({"$schema":"ara://schema/scene/1","id":"sc_profilernative","records":{}}),
        ),
    )?;
    std::fs::write(
        project.join(".ara-profiler-fixture"),
        "Ara isolated native Profiler fixture\n",
    )?;
    options.project = project;
    options.output = output;
    Ok(())
}
pub(super) fn setup(editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
    if editor.play_world.is_some() {
        return Err("Profiler fixture must open without a previous Play world".into());
    }
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":"e_profilercamera","name":"Profiler camera","components":{"Transform":{"position":[0,2,-5]},"Camera":{"priority":100}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_profilerbox","name":"Profiler sample","components":{"Transform":{"position":[0,0.5,0]},"MeshRenderer":{}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_profilerlight","name":"Profiler light","components":{"Transform":{"position":[0,4,-2]},"Light":{"intensity":1000}}}}),
    ],"Create native Profiler fixture",None)?;
    for (key, value) in [
        ("play.mode", json!("host")),
        ("play.clients", json!(2)),
        ("play.processMode", json!("separate")),
        ("play.inGameView", json!(true)),
        ("play.startPaused", json!(false)),
        ("rendering.quality", json!("low")),
    ] {
        editor.action("editor.settings.set", json!({"key":key,"value":value}))?;
    }
    editor.action("editor.preferences", json!({"reduceMotion":true}))?;
    editor.action("editor.layout.reset", json!({}))?;
    shell.rebuild(editor)?;
    Ok(())
}
