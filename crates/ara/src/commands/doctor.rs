//! Tester prerequisites, independent of a project or native build toolchain.
use serde_json::json;
use std::process::Command;

pub fn doctor(as_json: bool) -> bool {
    let probe = |args: &[&str]| {
        Command::new("dotnet")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default()
    };
    let sdk = probe(&["--list-sdks"])
        .lines()
        .any(|line| line.starts_with("10."));
    let runtime = probe(&["--list-runtimes"])
        .lines()
        .any(|line| line.starts_with("Microsoft.NETCore.App 10."));
    let tools = ara_shader_cook::Toolchain::discover();
    let slang = Command::new(&tools.slangc)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    let validator = Command::new(&tools.spirv_val)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    // Ara's own C# host and compile server: shipped with an installed engine, built by
    // `dotnet build` in a source checkout. Without them no project with scripts can play.
    let csharp = std::env::current_dir()
        .ok()
        .and_then(|here| ara_play_host::ScriptPaths::locate(&here));
    let checks = json!([
        {"name":".NET 10 SDK", "ok":sdk, "fix":"Install the x64 .NET 10 SDK: https://dotnet.microsoft.com/en-us/download/dotnet/10.0"},
        {"name":".NET 10 runtime", "ok":runtime, "fix":"The x64 .NET 10 SDK includes this runtime: https://dotnet.microsoft.com/en-us/download/dotnet/10.0"},
        {"name":"Ara C# tools", "ok":csharp.is_some(), "path":csharp.as_ref().map(|tools| &tools.host_dir), "fix":"From a source checkout, run `dotnet build dotnet/Ara.slnx -c Release` at the repository root (or set ARA_DOTNET_ARTIFACTS to its artifacts/bin). For an installed engine, repair the installation."},
        {"name":"Slang shader compiler", "ok":slang, "path":tools.slangc, "fix":"Reinstall Ara's tools, or install the Vulkan SDK and set VULKAN_SDK. Missing msvcp140/vcruntime140 DLLs: install Microsoft VC++ v14 x64 from https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist"},
        {"name":"SPIR-V validator", "ok":validator, "path":tools.spirv_val, "fix":"Reinstall Ara's tools, or install the Vulkan SDK and set VULKAN_SDK. Missing msvcp140/vcruntime140 DLLs: install Microsoft VC++ v14 x64 from https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist"}
    ]);
    let ok = sdk && runtime && csharp.is_some() && slang && validator;
    if as_json {
        println!(
            "{}",
            ara_json::to_canonical_string(&json!({"ok":ok,"checks":checks}))
        );
    } else {
        for check in checks.as_array().into_iter().flatten() {
            println!(
                "{} {}",
                if check["ok"] == true { "OK" } else { "MISSING" },
                check["name"].as_str().unwrap_or_default()
            );
            if check["ok"] != true {
                println!("  {}", check["fix"].as_str().unwrap_or_default());
            }
        }
        println!(
            "Graphics: Windows 10/11 x64, Vulkan 1.3 GPU/driver; ray tracing recommended. RAM: 16 GB. GPU launch is verified by opening the editor."
        );
    }
    ok
}
