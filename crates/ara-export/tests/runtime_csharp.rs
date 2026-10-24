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

#[test]
#[ignore = "requires a prebuilt ara-player and matching PDB; run explicitly with ARA_SHIPPING_E2E_PLAYER/PDB"]
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
    let module = author.join("scripts/Gameplay");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::create_dir_all(author.join("assets")).unwrap();
    write(
        &author.join("ara.project.json"),
        json!({"name":"Native CSharp Export","main":"main.scene"}),
    );
    std::fs::write(
        module.join("ara.module.toml"),
        "[module]\nname = \"Gameplay\"\n",
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
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        Ticks++;
        Get<Transform>().Rotation.Y += 2;
    }
}
"#,
    )
    .unwrap();
    write(
        &module.join("Gameplay.types.json"),
        json!({"$schema":"ara://schema/types/1","types":{
        "Spinner":{"kind":"component","module":"Gameplay","doc":"Counts fixed ticks and rotates its object.",
        "fields":{"ticks":{"type":"i32","doc":"Completed fixed ticks.","default":0}}}}}),
    );
    write(
        &author.join("main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_shipping1","records":{
            "e_spinner":{"name":"Spinner","order":"a0","Transform":{},"Spinner":{"ticks":0},
                "MeshRenderer":{"color":[0.2,0.7,0.9,1]}},
            "e_camera":{"name":"Camera","order":"a1","Transform":{"position":[0,1,5],"rotation":[-8,0,0]},"Camera":{}},
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
    let preset: ExportPreset = serde_json::from_value(json!({"platform":"windows","config":"dev",
        "playerTemplate":"runtime","lane":"nativeAot","assembly":"ShippingGame","content":"assets",
        "templates":[template_pin]}))
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
    let moved = scratch.join("relocated");
    std::fs::rename(output.join("build"), &moved).unwrap();
    let source = std::fs::canonicalize(&author).unwrap();
    assert!(source.starts_with(std::fs::canonicalize(&scratch).unwrap()));
    std::fs::remove_dir_all(source).unwrap();
    assert!(moved.join("game/ShippingGame.dll").is_file());
    assert!(moved.join("game/scripts.json").is_file());
    let log = scratch.join("relocated.log");
    let result = tools::run(
        Command::new(moved.join("ara-player.exe"))
            .args([
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
            ])
            .current_dir(&moved),
        &log,
        Duration::from_secs(120),
    )
    .unwrap();
    let result = verify::runtime_result(&result).unwrap();
    assert_eq!(result["tick"], json!(6));
    assert_eq!(result["reads"]["e_spinner.Spinner.ticks"], json!(6));
    assert_eq!(
        result["reads"]["e_spinner.Transform.rotation.y"],
        json!(12.0)
    );
    write(&scratch.join("result.json"), result);
    let manifest_path = moved.join("game/scripts.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let mut stale = manifest.clone();
    stale["layoutHash"] = "0000000000000000".into();
    write(&manifest_path, stale);
    let mismatch_log = scratch.join("mismatch.log");
    let failed = tools::run(
        Command::new(moved.join("ara-player.exe"))
            .args(["--headless", "--ticks", "1"])
            .current_dir(&moved),
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
