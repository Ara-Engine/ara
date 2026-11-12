#![allow(clippy::unwrap_used)]
//! A real headless child: C# settings -> native host -> graphics state and user storage, with
//! no gameplay state change, then save and load in a fresh player. The renderer is Renderlab,
//! which does not take the quality plan (shadow resolution and the rest): the setting reaches
//! the player's graphics state and storage, not the picture (see the bridge's gap table).
mod support;
use serde_json::json;
use support::{Agent, build_dotnet, repo};

#[test]
fn csharp_shadow_override_reaches_graphics_state_and_user_storage() {
    build_dotnet();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = repo()
        .join("target/player-graphics")
        .join(format!("{}-{nonce}", std::process::id()));
    let content = root.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(&json!({
            "name":"Graphics test", "main":"main.scene", "layoutVersion":4,
            "rendering":{"quality":"low","rayTracing":false}
        })),
    )
    .unwrap();
    std::fs::write(
        content.join("Options.cs"),
        r#"
/// <summary>Local options menu, with no ECS gameplay writes.</summary>
[Command("options.shadows")]
public static partial class ShadowMenu {
    /// <summary>Changes only the user's shadow group.</summary>
    public static CommandResult Execute(ref CommandContext ctx) {
        var candidate = GraphicsSettings.Pending;
        GraphicsSettings.Pending = candidate with {
            Preset = GraphicsPreset.Low,
            Groups = candidate.Groups with { Shadows = GraphicsTier.Epic }
        };
        GraphicsSettings.Apply();
        GraphicsSettings.Save();
        return CommandResult.Ok;
    }
}
"#,
    )
    .unwrap();
    let scene_id = format!("sc_g{nonce}");
    std::fs::write(content.join("main.scene"), ara_json::to_canonical_string(&json!({
        "$schema":"ara://schema/scene/1", "id":scene_id,
        "records":{
            "e_environment":{"name":"Environment","Environment":{"intensity":120,"exposure":0.002}},
            "e_ground":{"name":"Ground","Transform":{"position":[0,-0.5,0]},"MeshRenderer":{"size":[8,1,8],"color":[0.5,0.5,0.5,1]}},
            "e_box":{"name":"Box","Transform":{},"MeshRenderer":{"color":[0.6,0.25,0.12,1]}},
            "e_camera":{"name":"Camera","Transform":{"position":[0,2,6],"rotation":[-15,0,0]},"Camera":{"fov":55}},
            "e_sun":{"name":"Sun","Transform":{"rotation":[-35,-20,0]},"Light":{"kind":"directional","intensity":2400}}
        }
    }))).unwrap();
    let mut agent = Agent::open(&root, 5, |config| {
        config.pool.windowed = 0;
    });
    agent.pool_ready();
    let world = agent.play("start", json!({"startPaused":true,"hotReload":false}))["world"]
        .as_str()
        .unwrap()
        .to_owned();
    let initial = agent.play("graphics", json!({"world":world}));
    assert_eq!(initial["state"]["activeRayTracing"], false);
    assert_eq!(initial["state"]["defaults"]["preset"], "low");
    let before = agent.play("capture", json!({"world":world}))["hash"].clone();
    let mut candidate = initial["state"]["applied"].clone();
    candidate["groups"]["shadows"] = json!("medium");
    agent.play(
        "graphics",
        json!({"world":world,"request":{"operation":"set","settings":candidate}}),
    );
    agent.play(
        "graphics",
        json!({"world":world,"request":{"operation":"apply"}}),
    );
    assert_eq!(
        before,
        agent.play("capture", json!({"world":world}))["hash"],
        "graphics requests never enter gameplay state"
    );
    let command = agent.play(
        "command",
        json!({"world":world,"name":"options.shadows","params":{}}),
    );
    assert!(
        command["ops"].as_array().is_some_and(Vec::is_empty),
        "options command writes no components: {command}"
    );
    // The normal gameplay-command envelope advances its replay sequence even
    // with no component writes. Compare graphics/rendering at that same boundary.
    let before_gpu = agent.play("capture", json!({"world":world}))["hash"].clone();
    let after = agent.play("capture", json!({"world":world,"path":root.join("epic-shadows.png"),"width":320,"height":180,"marks":0}));
    assert_eq!(
        before_gpu, after["hash"],
        "user preferences must not change simulation state"
    );
    let state = agent.play("graphics", json!({"world":world}));
    assert_eq!(state["state"]["applied"]["groups"]["shadows"], "epic");
    assert_eq!(state["state"]["capabilities"]["deviceKnown"], true);
    // A capture with a per-capture preset, and one back at the user's settings, each give a
    // picture and a hash, and neither changes the saved preferences.
    for (name, preset) in [
        ("medium-capture.png", Some("medium")),
        ("restored-low.png", None),
    ] {
        let mut request =
            json!({"world":world,"path":root.join(name),"width":320,"height":180,"marks":0});
        if let Some(preset) = preset {
            request["preset"] = json!(preset);
        }
        let shot = agent.play("capture", request);
        assert!(shot["hash"].is_string(), "{name} has a hash: {shot}");
        assert!(root.join(name).is_file(), "{name} has a picture");
        assert_eq!(
            agent.play("graphics", json!({"world":world}))["state"]["applied"]["groups"]["shadows"],
            "epic",
            "{name} leaves the user's shadow group alone"
        );
    }
    let ray_supported = state["state"]["capabilities"]["rayTracingSupported"]
        .as_bool()
        .unwrap();
    if ray_supported {
        let mut candidate = state["state"]["applied"].clone();
        candidate["rayTracing"] = json!(true);
        agent.play(
            "graphics",
            json!({"world":world,"request":{"operation":"set","settings":candidate}}),
        );
        let applied = agent.play(
            "graphics",
            json!({"world":world,"request":{"operation":"apply"}}),
        );
        assert_eq!(applied["state"]["restartRequired"], true);
        assert_eq!(applied["state"]["activeRayTracing"], false);
        agent.play(
            "graphics",
            json!({"world":world,"request":{"operation":"save"}}),
        );
    }
    agent.play("stop", json!({"world":world}));
    let fresh = agent.play("start", json!({"startPaused":true,"hotReload":false}))["world"]
        .as_str()
        .unwrap()
        .to_owned();
    let loaded = agent.play("graphics", json!({"world":fresh}));
    assert_eq!(
        loaded["state"]["activeRayTracing"], ray_supported,
        "saved RT opt-in wins over the pool's warm-up policy"
    );
    assert_eq!(loaded["state"]["applied"]["groups"]["shadows"], "epic");
    agent.play(
        "graphics",
        json!({"world":fresh,"request":{"operation":"resetToDefaults"}}),
    );
    agent.play(
        "graphics",
        json!({"world":fresh,"request":{"operation":"apply"}}),
    );
    let rejected = agent.raw("ara_play", json!({"action":"graphics","world":fresh,"request":{"operation":"set","settings":{"hdr":true}}}));
    assert_eq!(rejected["isError"], false);
    let rejected = agent.raw(
        "ara_play",
        json!({"action":"graphics","world":fresh,"request":{"operation":"apply"}}),
    );
    assert_eq!(
        rejected["isError"], true,
        "headless surfaces must reject HDR output"
    );
    agent.play("stop", json!({"world":fresh}));
}
