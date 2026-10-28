#![allow(clippy::unwrap_used)]
//! Exports actual generated gameplay, relocates it, removes source, then runs the
//! packed scene through the same runtime plugin as the standalone player.
use ara_export::{
    scripts,
    tools::{self, Toolchain},
};
use ara_play::{PlayScene, ScriptScene};
use ara_runtime::{Runtime, RuntimeConfig, assets::MemoryAssets};
use ara_runtime_script::ScriptPlugin;
use ara_script::shipping::ScriptManifest;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

fn runtime(scene: &PlayScene) -> ara_runtime::Result<Runtime> {
    let scripts = scene.scripts.as_ref().unwrap();
    let ara_play::ScriptHost::NativeAot { manifest } = &scripts.host else {
        panic!("shipping mode")
    };
    let (plugin, _) = ScriptPlugin::shipping(manifest);
    let (workspace, id) = scene.workspace().unwrap();
    Runtime::new(
        ara_runtime::schema(&scene.types)?,
        workspace,
        id,
        Box::<MemoryAssets>::default(),
        RuntimeConfig::default(),
        vec![Box::new(plugin)],
    )
}

#[test]
fn exported_nativeaot_behaviour_runs_without_sources_and_rejects_stale_bindings() {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .join("shipping-scripts");
    std::fs::create_dir_all(&target).unwrap();
    let scratch = tempfile::tempdir_in(&target).unwrap();
    let project = scratch.path().join("author");
    let module = project
        .join(ara_doc::AUTHORING_ROOT)
        .join("scripts/Gameplay");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(
        project.join("ara.project.json"),
        ara_json::to_canonical_string(&json!({"name":"Shipping fixture","layoutVersion":3})),
    )
    .unwrap();
    std::fs::create_dir_all(module.join("nested/editor")).unwrap();
    std::fs::write(
        module.join("nested/editor/Tools.cs"),
        "#error editor sources must never compile into the player",
    )
    .unwrap();
    let core = project.join(ara_doc::AUTHORING_ROOT).join("scripts/Core");
    std::fs::create_dir_all(&core).unwrap();
    std::fs::write(core.join("core.module.json"), r#"{"name":"Core"}"#).unwrap();
    std::fs::write(
        core.join("SpinRate.cs"),
        r#"
namespace Core;
/// <summary>Authored turn rate shared with gameplay.</summary>
[Component] public partial struct SpinRate
{
    /// <summary>Degrees per fixed tick.</summary>
    public float Degrees;
}
"#,
    )
    .unwrap();
    std::fs::write(
        module.join("gameplay.module.json"),
        r#"{"name":"Gameplay","references":["Core"]}"#,
    )
    .unwrap();
    std::fs::write(
        module.join("Spinner.cs"),
        r#"
namespace Game;
/// <summary>Counts fixed ticks and turns the authored object.</summary>
public sealed partial class Spinner : Behaviour
{
    /// <summary>Completed gameplay ticks.</summary>
    [Property] public partial int Ticks { get; set; }
    /// <summary>Owner returned by this world's input callback.</summary>
    [Property] public partial ulong Owner { get; set; }
    /// <summary>Axis returned by the fixed-tick input callback.</summary>
    [Property] public partial float Axis { get; set; }
    /// <summary>Lights returned by the reflected component query.</summary>
    [Property] public partial int Lights { get; set; }
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        Ticks++;
        Get<Transform>().Rotation.Y += Get<Core.SpinRate>().Degrees;
        Get<Light>().Intensity = Ticks;
        Owner = Input.Player(Entity);
        Axis = Input.Axis2(Owner, "Move").X;
        Span<Entity> scratch = stackalloc Entity[4];
        Lights = World.Query<Light>(scratch).Length;
        Navigation.GoTo(Entity, new Vector3(Ticks, 0, 0));
        Movement.Stop(Entity);
        Animation.SetBool(Entity, "Moving", true);
    }
}
"#,
    )
    .unwrap();
    let job = scratch.path().join("job");
    let toolchain = Toolchain::discover();
    let shipping = scripts::prepare(&project, "GameplayNative", &job, &toolchain)
        .unwrap()
        .unwrap();
    let publish = job.join("publish");
    let rid = if cfg!(windows) {
        "win-x64"
    } else if cfg!(target_os = "macos") {
        "osx-arm64"
    } else {
        "linux-x64"
    };
    tools::run(
        Command::new("dotnet")
            .arg("publish")
            .arg(&shipping.project)
            .args([
                "-c",
                "Release",
                "-r",
                rid,
                "--nologo",
                "--disable-build-servers",
                "-o",
            ])
            .arg(&publish)
            .args([
                "-p:PublishAot=true",
                "-p:NativeLib=Shared",
                "-p:UseSharedCompilation=false",
            ])
            .env("PATH", toolchain.path().unwrap()),
        &job.join("logs/publish.log"),
        Duration::from_secs(300),
    )
    .unwrap();
    let library = format!(
        "GameplayNative.{}",
        if cfg!(windows) {
            "dll"
        } else if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        }
    );
    let export = scratch.path().join("export");
    std::fs::create_dir_all(export.join("game")).unwrap();
    std::fs::copy(publish.join(&library), export.join("game").join(&library)).unwrap();
    scripts::write_manifest(&export.join("game"), &library, &shipping.modules).unwrap();
    // Documents may carry script columns before module registration at prepare.
    let scene: PlayScene = serde_json::from_value(json!({
        "format":1,"scene":"sc_shipping","documents":[{"path":"main.scene","kind":"scene","id":"sc_shipping",
        "records":{"e_spinner":{"Transform":{},"Light":{},"PlayerControl":{"player":7},"SpinRate":{"degrees":2},"Spinner":{"ticks":0}}}}],"types":[],"seed":7,"tick_hz":60,
        "assets":{},"cache":"","nav_cache":null,"scripts":ScriptScene::shipping("game/scripts.json"),"imports":{}
    })).unwrap();
    let packed = ara_pack::build(
        [ara_pack::Input {
            path: ".ara/scene.cbor".into(),
            bytes: scene.encode().unwrap(),
        }],
        ara_pack::CookKey {
            platform: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            config: "release".into(),
            cooker: "shipping-test/1".into(),
            settings: BTreeMap::new(),
        },
        1024 * 1024,
        None,
    )
    .unwrap();
    std::fs::write(export.join("game.arapack"), &packed.bytes).unwrap();
    let relocated = scratch.path().join("moved-export");
    std::fs::rename(&export, &relocated).unwrap();
    // Delete only the explicitly created scratch author folder.
    let source = std::fs::canonicalize(&project).unwrap();
    assert!(source.starts_with(std::fs::canonicalize(scratch.path()).unwrap()));
    std::fs::remove_dir_all(source).unwrap();
    let bytes = std::fs::read(relocated.join("game.arapack")).unwrap();
    let pack = ara_pack::Pack::open(&bytes).unwrap();
    let mut loaded =
        PlayScene::decode(&pack.read(".ara/scene.cbor", 1024 * 1024, None).unwrap()).unwrap();
    loaded
        .scripts
        .as_mut()
        .unwrap()
        .resolve_shipping(&relocated);
    let manifest_path = relocated.join("game/scripts.json");
    let valid = ScriptManifest::read(&manifest_path).unwrap();
    for kind in ["abi", "layout"] {
        let mut wrong = valid.clone();
        if kind == "abi" {
            wrong.abi_version += 1;
        } else {
            wrong.layout_hash = "0000000000000000".into();
        }
        tools::write_json(&manifest_path, &serde_json::to_value(wrong).unwrap()).unwrap();
        let error = runtime(&loaded)
            .err()
            .expect("mismatched shipping bindings rejected");
        assert!(
            error.0.contains("ARA0921") && error.0.contains("ABI/layout mismatch"),
            "{error}"
        );
    }
    tools::write_json(&manifest_path, &serde_json::to_value(valid).unwrap()).unwrap();
    let mut game = runtime(&loaded).unwrap();
    let mut second = runtime(&loaded).unwrap();
    let second_entity = second.play().session().entity("e_spinner").unwrap();
    second
        .play_mut()
        .session_mut()
        .world_mut()
        .get_mut::<ara_runtime::components::PlayerControl>(second_entity)
        .unwrap()
        .player = 9;
    game.step(6, &[]).unwrap();
    let session = game.play().session();
    let entity = session.entity("e_spinner").unwrap();
    assert_eq!(
        ara_runtime_script::read(session.world(), entity, "Spinner"),
        Some(json!({"ticks":6,"owner":7,"axis":0.0,"lights":1}))
    );
    assert_eq!(
        session
            .component("e_spinner", "Transform")
            .unwrap()
            .unwrap()["rotation"][1],
        json!(12.0)
    );
    assert!(
        !ara_runtime_script::request_compile(
            session.world(),
            "Gameplay",
            &[PathBuf::from("Spinner.cs")]
        ),
        "shipping never starts Roslyn"
    );
    let snapshot = game.play().snapshot().unwrap();
    game.step(3, &[]).unwrap();
    game.restore(&snapshot).unwrap();
    let session = game.play().session();
    assert_eq!(
        ara_runtime_script::read(
            session.world(),
            session.entity("e_spinner").unwrap(),
            "Spinner"
        ),
        Some(json!({"ticks":6,"owner":7,"axis":0.0,"lights":1}))
    );
    drop(game);
    // The same generated shipping entry owns a second host, still callable after
    // the first is destroyed, beyond the old 2.7-second failure window.
    second.step(240, &[]).unwrap();
    assert_eq!(
        ara_runtime_script::read(second.world(), second_entity, "Spinner"),
        Some(json!({"ticks":240,"owner":9,"axis":0.0,"lights":1}))
    );
    assert_eq!(
        second
            .play()
            .session()
            .component("e_spinner", "Light")
            .unwrap()
            .unwrap()["intensity"],
        json!(240.0)
    );
}
