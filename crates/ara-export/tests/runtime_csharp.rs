#![cfg(windows)]
#![allow(clippy::unwrap_used)]
//! Real export pipeline and deployed player, including symbols, cooked resources,
//! packed script selection, relocation and observable NativeAOT behavior ticks.
use ara_export::{ExportPreset, ExportRequest, event::EventKind, tools, verify};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

fn write(path: &Path, value: Value) {
    tools::write_json(path, &value).unwrap();
}

fn pin(path: &Path) -> Value {
    json!({"hash":blake3::hash(&std::fs::read(path).unwrap()).to_hex().to_string(),
        "source":path.to_str().unwrap()})
}

/// Launch the named application from an unrelated working directory with only
/// OS runtime paths available; the shipped binary must find its own data.
fn deployed_command(game: &Path, report: &Path) -> Command {
    let mut command = Command::new(game.join("ShippingGame.exe"));
    deployed_environment(&mut command, report);
    command
}

fn deployed_environment(command: &mut Command, report: &Path) {
    let windows = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system root"));
    for name in ["profile", "crashes", "temp", "local", "roaming"] {
        std::fs::create_dir_all(report.join(name)).unwrap();
    }
    command
        .current_dir(report)
        .env(
            "PATH",
            std::env::join_paths([windows.join("System32"), windows]).unwrap(),
        )
        .env("ARA_CONFIG_DIR", report.join("profile"))
        .env("ARA_CRASH_DIR", report.join("crashes"))
        .env("LOCALAPPDATA", report.join("local"))
        .env("APPDATA", report.join("roaming"))
        .env("TEMP", report.join("temp"))
        .env("TMP", report.join("temp"))
        .env("ARA_DOTNET_SDK", report.join("unavailable-sdk"))
        .env("ARA_DOTNET_ARTIFACTS", report.join("unavailable-sdk"))
        .env("DOTNET_ROOT", report.join("unavailable-dotnet"))
        .env("VK_LAYER_PATH", report.join("empty-layers"))
        .env("VK_IMPLICIT_LAYER_PATH", report.join("empty-layers"))
        .env_remove("ARA_SHADER_DIR")
        .env_remove("ARA_SHADER_PACKAGE")
        .env_remove("VULKAN_SDK")
        .env_remove("SLANGC");
}

#[test]
#[ignore = "requires a prebuilt release ara-player and matching PDB; run explicitly with ARA_SHIPPING_E2E_PLAYER/PDB"]
fn exported_game_ticks_csharp_after_relocation_without_author_files() {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let evidence = engine.join("target/shipping-integration");
    std::fs::create_dir_all(&evidence).unwrap();
    // Keep this complete runnable export as reviewable evidence under target/.
    let scratch = tempfile::Builder::new()
        .prefix("runtime-csharp-")
        .tempdir_in(&evidence)
        .unwrap()
        .keep();
    let player =
        PathBuf::from(std::env::var_os("ARA_SHIPPING_E2E_PLAYER").expect("prebuilt ara-player"));
    let pdb = PathBuf::from(std::env::var_os("ARA_SHIPPING_E2E_PDB").expect("matching player PDB"));
    let author = scratch.join("author");
    let authored = author.clone();
    // C# retains its compiler-discovery root while assets exercise the legacy layout.
    let module = author
        .join(ara_doc::AUTHORING_ROOT)
        .join("scripts/Gameplay");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::create_dir_all(authored.join("scenes")).unwrap();
    std::fs::create_dir_all(authored.join("prefabs")).unwrap();
    std::fs::create_dir_all(authored.join("ui")).unwrap();
    std::fs::write(
        authored.join("ui/game.ui"),
        "<ui id=\"game\"><text id=\"ticks\" text=\"Ticks\"/></ui>",
    )
    .unwrap();
    std::fs::write(authored.join("ui/game.css"), "").unwrap();
    write(
        &authored.join("ui/bindings.json"),
        json!([
            {"node":"ticks","entity":"e_spinner","component":"Spinner","field":"ticks"}
        ]),
    );
    write(
        &author.join("ara.project.json"),
        json!({"name":"Native CSharp Export","main":"scenes/main.scene"}),
    );
    std::fs::write(
        module.join("gameplay.module.json"),
        r#"{"name":"Gameplay"}"#,
    )
    .unwrap();
    std::fs::write(
        module.join("Spinner.cs"),
        r#"
namespace Game;
/// <summary>Counts fixed ticks and rotates its object.</summary>
public sealed partial class Spinner : Behaviour
{
    /// <summary>Completed fixed ticks.</summary>
    [Property] public partial int Ticks { get; set; }
    /// <summary>Owner sampled through the shipping callback table.</summary>
    [Property] public partial ulong Owner { get; set; }
    /// <summary>Number of renderable targets found through the shipping query.</summary>
    [Property] public partial int Targets { get; set; }
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        Ticks++;
        Get<Transform>().Rotation.Y += 2;
        Owner = Input.Player(Entity);
        Span<Entity> scratch = stackalloc Entity[4];
        Targets = World.Query<MeshRenderer>(scratch).Length;
        Navigation.GoTo(Entity, new Vector3(Input.Axis2(Owner, "Move").X, 0, 0));
        Movement.Stop(Entity);
        Animation.SetBool(Entity, "Moving", true);
    }
}
"#,
    )
    .unwrap();
    write(
        &authored.join("prefabs/anchor.prefab"),
        json!({"$schema":"ara://schema/prefab/1","id":"pf_shippinganchor","records":{
            "e_base":{"name":"Authored anchor","Transform":{"position":[3,0,0]}}
        }}),
    );
    write(
        &authored.join("scenes/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_shipping1","records":{
            "e_spinner":{"name":"Spinner","order":"a0","Transform":{},"PlayerControl":{"player":7},"Spinner":{"ticks":0},
                "MeshRenderer":{"color":[0.2,0.7,0.9,1]}},
            "e_camera":{"name":"Camera","order":"a1","Transform":{"position":[0,1,5],"rotation":[-8,0,0]},"Camera":{}},
            "e_anchor":{"order":"a3","prefab":"@prefabs/anchor.prefab#pf_shippinganchor"},
            "e_sun":{"name":"Sun","order":"a2","Transform":{"rotation":[-35,-30,0]},"Light":{"kind":"directional","intensity":10000}}
        }}),
    );
    let template = scratch.join("runtime.zip");
    ara_export::build_runtime_template(&player, &engine, &template).unwrap();
    let mut template_pin = pin(&template);
    template_pin["platform"] = "windows".into();
    template_pin["arch"] = "x64".into();
    template_pin["config"] = "dev".into();
    template_pin["symbols"] = pin(&pdb);
    let preset: ExportPreset = serde_json::from_value(
        json!({"platform":"windows","config":"dev","name":"ShippingGame",
        "playerTemplate":"runtime","lane":"nativeAot","assembly":"ShippingGame","content":".",
        "templates":[template_pin]}),
    )
    .unwrap();
    let output = scratch.join("export");
    let mut events = Vec::new();
    let summary = ara_export::export(
        &ExportRequest {
            project: author.clone(),
            preset,
            output: output.clone(),
            cache: scratch.join("cache"),
            device: None,
            allow_emulator: false,
        },
        &mut events,
    )
    .unwrap();
    assert!(summary.verify.runtime_executed && summary.verify.screenshot.is_some());
    assert!(events.iter().any(|e| e.stage == "script"
        && matches!(&e.kind, EventKind::Progress { status, .. } if status == "completed")));
    assert!(events.iter().any(|e| e.stage == "pack"
        && matches!(&e.kind, EventKind::Progress { status, .. } if status == "completed")));
    let pack_bytes = std::fs::read(output.join("build/data/game.arapack")).unwrap();
    let pack = ara_pack::Pack::open(&pack_bytes).unwrap();
    let snapshot = ara_play::PlayScene::decode(
        &pack
            .read(".ara/scene.cbor", 64 * 1024 * 1024, None)
            .unwrap(),
    )
    .unwrap();
    assert!(snapshot.cache.is_empty() && snapshot.nav_cache.is_none());
    let (workspace, scene_id) = snapshot.workspace().unwrap();
    let mut diagnostics = Vec::new();
    let resolved = workspace.resolve(
        &scene_id,
        &ara_runtime::schema(&snapshot.types).unwrap(),
        &mut diagnostics,
    );
    assert!(
        diagnostics.is_empty(),
        "packed authored-relative reference diagnostics: {diagnostics:?}"
    );
    assert_eq!(resolved["e_anchor"].record["name"], "Authored anchor");
    let scripts = snapshot
        .scripts
        .as_ref()
        .expect("compiled gameplay in the pack");
    assert!(
        scripts.host_dir.is_empty()
            && scripts.compile_server.is_none()
            && scripts.modules.is_empty()
    );
    assert!(
        matches!(&scripts.host, ara_play::ScriptHost::NativeAot { manifest } if manifest == "game/scripts.json")
    );
    // The release gate supplies an external target/ root to prove the installed
    // cohort outside this checkout. Local opt-in runs can retain their own evidence.
    let moved = if let Some(base) = std::env::var_os("ARA_SHIPPING_RELOCATION_ROOT") {
        let base = std::path::absolute(PathBuf::from(base)).unwrap();
        std::fs::create_dir_all(&base).unwrap();
        let base = dunce::canonicalize(base).unwrap();
        assert!(
            !base.starts_with(dunce::canonicalize(&engine).unwrap()),
            "relocation must be outside the developer checkout"
        );
        tempfile::Builder::new()
            .prefix("Ara shipping 雪 space ")
            .tempdir_in(base)
            .unwrap()
            .keep()
            .join("game")
    } else {
        scratch.join("relocated")
    };
    std::fs::rename(output.join("build"), &moved).unwrap();
    let deployed_report = moved.parent().unwrap().join("launch-report");
    std::fs::create_dir_all(deployed_report.join("empty-layers")).unwrap();
    let source = std::fs::canonicalize(&author).unwrap();
    assert!(source.starts_with(std::fs::canonicalize(&scratch).unwrap()));
    std::fs::remove_dir_all(source).unwrap();
    assert!(moved.join("data/game/ShippingGame.dll").is_file());
    assert!(moved.join("data/game/scripts.json").is_file());
    assert!(!moved.join("verify.png").exists());
    assert!(output.join("report/verify.png").is_file());
    let log = scratch.join("relocated.log");
    let result = tools::run(
        deployed_command(&moved, &deployed_report).args([
            "--headless",
            "--ticks",
            "6",
            "--quality",
            "low",
            "--size",
            "320x180",
            "--read",
            "e_spinner.Spinner.ticks",
            "--read",
            "e_spinner.Transform.rotation.y",
            "--read",
            "e_spinner.Spinner.owner",
            "--read",
            "e_spinner.Spinner.targets",
        ]),
        &log,
        Duration::from_secs(120),
    )
    .unwrap();
    let result = verify::runtime_result(&result).unwrap();
    assert!(
        !moved.join("data/verify.png").exists(),
        "headless gameplay must not write inside shipped data"
    );
    assert!(
        !moved.join("verify.png").exists(),
        "headless gameplay must not write inside the installation"
    );
    assert_eq!(result["tick"], json!(6));
    assert_eq!(result["reads"]["e_spinner.Spinner.ticks"], json!(6));
    assert_eq!(result["reads"]["e_spinner.Spinner.owner"], json!(7));
    assert_eq!(result["reads"]["e_spinner.Spinner.targets"], json!(1));
    assert_eq!(
        result["reads"]["e_spinner.Transform.rotation.y"],
        json!(12.0)
    );
    write(&scratch.join("result.json"), result);
    // Shipping verification used to exercise only headless captures, missing the
    // adapter-owned scene image and error teardown in continuous native UI.
    let windowed = tools::run(
        deployed_command(&moved, &deployed_report).args([
            "--offscreen",
            "--frames",
            "240",
            "--quality",
            "low",
            "--size",
            "320x180",
            "--read",
            "e_spinner.Spinner.owner",
            "--read",
            "e_spinner.Spinner.targets",
        ]),
        &scratch.join("continuous-ui.log"),
        Duration::from_secs(120),
    )
    .unwrap();
    let windowed = verify::runtime_result(&windowed).unwrap();
    assert_eq!(windowed["frames_after_first"], json!(239));
    assert!(windowed["tick"].as_u64().unwrap() > 0);
    assert_eq!(windowed["reads"]["e_spinner.Spinner.owner"], json!(7));
    assert_eq!(windowed["reads"]["e_spinner.Spinner.targets"], json!(1));
    write(&scratch.join("continuous-ui.json"), windowed);
    let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let no_arguments = deployed_report.join("no-arguments.json");
    // Windows PowerShell's legacy C# compiler cannot encode a Unicode TEMP
    // response path. Keep its scratch ASCII; the game and working dir stay Unicode.
    let compiler_temp = scratch.join("powershell-temp");
    std::fs::create_dir_all(&compiler_temp).unwrap();
    let mut launch = Command::new(powershell);
    deployed_environment(&mut launch, &deployed_report);
    launch
        .env("TEMP", &compiler_temp)
        .env("TMP", &compiler_temp)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(engine.join("release/test-isolated-game-launch.ps1"))
        .arg("-Exe")
        .arg(moved.join("ShippingGame.exe"))
        .arg("-WorkingDirectory")
        .arg(&deployed_report)
        .arg("-Report")
        .arg(&no_arguments)
        .args(["-ExpectedTitle", "ShippingGame"]);
    tools::run(
        &mut launch,
        &scratch.join("no-arguments.log"),
        Duration::from_secs(120),
    )
    .unwrap();
    let launched: Value = serde_json::from_slice(&std::fs::read(no_arguments).unwrap()).unwrap();
    assert_eq!(launched["passed"], true);
    assert_eq!(launched["arguments"], json!([]));
    assert_eq!(launched["visibleDesktopUsed"], false);
    assert_eq!(launched["doubleClickVerified"], false);
    assert_eq!(launched["title"], "ShippingGame");
    assert_eq!(launched["windowMessageBoundMs"], 1500);
    assert_eq!(launched["probeGuardMs"], 5000);
    for field in ["maxPumpLatencyMs", "maxProbeDurationMs"] {
        assert!(
            launched[field]
                .as_f64()
                .is_some_and(|ms| ms.is_finite() && ms >= 0.0),
            "invalid {field}: {}",
            launched[field]
        );
    }
    let iterations = launched["iterations"]
        .as_array()
        .expect("per-iteration probe evidence");
    assert!(
        !iterations.is_empty(),
        "launch proof must contain observed probes"
    );
    let mut latencies = Vec::new();
    let mut responsive_sample = false;
    for sample in iterations {
        assert!(
            sample["probeDurationMs"]
                .as_f64()
                .is_some_and(|ms| ms.is_finite() && ms >= 0.0),
            "invalid probe duration: {sample}"
        );
        let titles = sample["windowTitles"]
            .as_array()
            .expect("observed window titles");
        let messages = sample["messages"].as_u64().expect("message count");
        let responsive = sample["responsive"].as_bool().expect("response state");
        if messages > 0 {
            let latency = sample["pumpLatencyMs"]
                .as_f64()
                .expect("sent WM_NULL has a latency sample");
            assert!(
                latency.is_finite() && latency >= 0.0,
                "invalid message latency: {sample}"
            );
            assert!(
                titles
                    .iter()
                    .any(|title| title.as_str() == Some("ShippingGame")),
                "message response must belong to ShippingGame: {sample}"
            );
            latencies.push(latency);
            responsive_sample |= responsive;
        } else {
            assert!(
                !responsive,
                "responsiveness requires an observed message: {sample}"
            );
            assert!(
                sample["pumpLatencyMs"].is_null(),
                "unprobed windows have no message latency: {sample}"
            );
        }
    }
    assert!(
        !latencies.is_empty(),
        "launch proof must contain finite message latency observations"
    );
    assert!(
        responsive_sample,
        "ShippingGame must answer an observed WM_NULL probe"
    );
    assert!(
        launched["desktop"]
            .as_str()
            .unwrap()
            .starts_with("ara-export-")
    );
    let manifest_path = moved.join("data/game/scripts.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let mut stale = manifest.clone();
    stale["layoutHash"] = "0000000000000000".into();
    write(&manifest_path, stale);
    let mismatch_log = scratch.join("mismatch.log");
    let failed = tools::run(
        deployed_command(&moved, &deployed_report).args(["--headless", "--ticks", "1"]),
        &mismatch_log,
        Duration::from_secs(30),
    );
    write(&manifest_path, manifest);
    assert!(failed.is_err(), "stale shipping metadata must fail");
    let diagnostic = std::fs::read_to_string(mismatch_log).unwrap();
    assert!(diagnostic.contains("ARA0921"), "{diagnostic}");
    assert!(diagnostic.contains("ABI/layout mismatch"), "{diagnostic}");
    println!("runtime C# export evidence: {}", scratch.display());
}
