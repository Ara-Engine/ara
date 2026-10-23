use crate::{
    Platform,
    tools::{self, Toolchain},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Whether a prerequisite is usable, optional or blocking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ready.
    Ok,
    /// Export possible but a capability is missing.
    Warning,
    /// Required prerequisite missing.
    Error,
}

/// One stable doctor result with runnable remediation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    /// Stable identifier retained on successful checks as well.
    pub code: String,
    /// Prerequisite status.
    pub status: Status,
    /// Description.
    pub message: String,
    /// Observed paths, versions or device counts.
    pub context: Value,
    /// Commands to install or configure prerequisites.
    pub fixes: Vec<String>,
}

/// Platform-scoped diagnostics, without starting WSL or an emulator.
#[derive(Debug, Clone)]
pub struct DoctorOptions {
    /// Check only this platform, or all platforms when absent.
    pub platform: Option<Platform>,
    /// Android target API configured by the preset.
    pub target_api: u32,
    /// Explicit opt-in to adb device enumeration.
    pub devices: bool,
    /// Scratch directory for bounded tool probe logs.
    pub scratch: PathBuf,
}

/// Version predicate used by probes and unit tests.
pub fn version_matches(text: &str, major: u32, minimum_minor: u32) -> bool {
    let v = tools::version(text);
    v.first() == Some(&major) && v.get(1).copied().unwrap_or(0) >= minimum_minor
}

/// Runs S7's toolchain checks as platform-scoped production diagnostics.
pub fn doctor(opts: &DoctorOptions, t: &Toolchain) -> Vec<Check> {
    let mut out = Vec::new();
    let mut add =
        |code: &str, ok: bool, optional: bool, message: &str, fix: String, context: Value| {
            out.push(Check {
                code: code.into(),
                status: if ok {
                    Status::Ok
                } else if optional {
                    Status::Warning
                } else {
                    Status::Error
                },
                message: message.into(),
                context,
                fixes: if ok { Vec::new() } else { vec![fix] },
            });
        };
    let probe = |p: &Path, args: &[&str], name: &str| {
        tools::probe(p, args, &opts.scratch, name).unwrap_or_default()
    };
    let dotnet = probe(Path::new("dotnet"), &["--version"], "dotnet");
    add(
        "ARA-EXP-SDK-001",
        version_matches(&dotnet, 10, 0),
        false,
        ".NET 10 SDK",
        "winget install Microsoft.DotNet.SDK.10".into(),
        json!({"version":dotnet.trim()}),
    );
    let rust = probe(Path::new("rustc"), &["--version"], "rust");
    add(
        "ARA-EXP-SDK-002",
        rust.contains("1.99.1"),
        true,
        "Rust 1.99.1 (template/native plugin development)",
        "rustup toolchain install 1.99.1".into(),
        json!({"version":rust.trim()}),
    );
    let targets = probe(
        Path::new("rustup"),
        &["target", "list", "--installed"],
        "targets",
    );
    add(
        "ARA-EXP-SDK-003",
        tools::which("cargo").is_some(),
        true,
        "Cargo (native plugin lane)",
        "rustup toolchain install 1.99.1".into(),
        json!({}),
    );
    let packages = std::env::var_os("NUGET_PACKAGES")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|p| PathBuf::from(p).join(".nuget/packages"))
        });
    let cached = |name: &str| packages.as_ref().is_some_and(|p| p.join(name).is_dir());
    if opts.platform.is_none_or(|p| p == Platform::Windows) {
        add("ARA-EXP-WIN-001", t.vswhere.is_some(), false,"MSVC Build Tools locator", "winget install Microsoft.VisualStudio.2022.BuildTools --override '--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive'".into(),json!({"path":t.vswhere}));
        let vc = t
            .vswhere
            .as_ref()
            .map(|p| {
                probe(
                    p,
                    &[
                        "-latest",
                        "-products",
                        "*",
                        "-requires",
                        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                        "-property",
                        "installationPath",
                    ],
                    "msvc",
                )
            })
            .unwrap_or_default();
        add("ARA-EXP-WIN-002", !vc.trim().is_empty(), false,"MSVC x64 compiler/linker and Windows SDK", "winget install Microsoft.VisualStudio.2022.BuildTools --override '--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive'".into(),json!({"installation":vc.trim()}));
        add(
            "ARA-EXP-WIN-003",
            cached("runtime.win-x64.microsoft.dotnet.ilcompiler"),
            true,
            "NativeAOT compiler package cached",
            "dotnet restore --runtime win-x64".into(),
            json!({}),
        );
        add(
            "ARA-EXP-WIN-004",
            targets.contains("x86_64-pc-windows-msvc"),
            true,
            "Windows runtime template target",
            "rustup target add x86_64-pc-windows-msvc".into(),
            json!({}),
        );
        add(
            "ARA-EXP-WIN-010",
            std::env::var_os("ARA_WIN_SIGN").is_some(),
            true,
            "Optional external Authenticode signing identity",
            "signtool sign /a /fd SHA256 dist/windows/ara-player.exe".into(),
            json!({"note":"Authenticode is an external post-export step; no identity is claimed by the exporter"}),
        );
    }
    if opts.platform.is_none_or(|p| p == Platform::Linux) {
        let zig = t
            .zig
            .as_ref()
            .map(|p| probe(p, &["version"], "zig"))
            .unwrap_or_default();
        add(
            "ARA-EXP-LNX-006",
            zig.trim() == "0.17.0",
            false,
            "Zig 0.17.0 pinned glibc 2.31 linker",
            "$env:ARA_ZIG='<path to zig 0.17.0>/zig.exe'".into(),
            json!({"version":zig.trim(),"target":"x86_64-linux-gnu.2.31"}),
        );
        add(
            "ARA-EXP-LNX-007",
            t.zigcc.is_some(),
            false,
            "Production ara-zigcc wrapper",
            "cargo build -p ara-export --bin ara-zigcc".into(),
            json!({"path":t.zigcc}),
        );
        add(
            "ARA-EXP-LNX-005",
            targets.contains("x86_64-unknown-linux-gnu"),
            true,
            "Linux runtime template target",
            "rustup target add x86_64-unknown-linux-gnu".into(),
            json!({}),
        );
        add(
            "ARA-EXP-LNX-011",
            cached("microsoft.netcore.app.runtime.linux-x64"),
            true,
            "ReadyToRun runtime cached",
            "dotnet restore --runtime linux-x64".into(),
            json!({}),
        );
        add(
            "ARA-EXP-LNX-012",
            t.ndk_bin()
                .is_some_and(|p| p.join(tools::executable("llvm-objcopy")).is_file()),
            true,
            "Optional llvm-objcopy for stripped symbols",
            r"$env:ANDROID_HOME='D:\Android\sdk'".into(),
            json!({"note":"export keeps symbols when objcopy is unavailable"}),
        );
    }
    if opts.platform.is_none_or(|p| p == Platform::Android) {
        add(
            "ARA-EXP-AND-001",
            t.sdk.is_some(),
            false,
            "Android SDK",
            r"$env:ANDROID_HOME='D:\Android\sdk'".into(),
            json!({"path":t.sdk}),
        );
        let ndk = t
            .ndk
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p.join("source.properties")).ok())
            .unwrap_or_default();
        let revision = ndk
            .lines()
            .find(|l| l.starts_with("Pkg.Revision"))
            .unwrap_or("");
        add(
            "ARA-EXP-AND-002",
            tools::version(revision).first().is_some_and(|v| *v >= 28)
                && t.ndk_bin()
                    .is_some_and(|p| p.join(tools::executable("clang")).is_file()),
            false,
            "NDK r28+ with 16 KB support",
            "sdkmanager 'ndk;28.2.13676358'".into(),
            json!({"revision":revision}),
        );
        for (name, code) in [
            ("aapt2", "ARA-EXP-AND-003"),
            ("zipalign", "ARA-EXP-AND-016"),
        ] {
            add(
                code,
                t.android_tool(name).is_some(),
                false,
                name,
                "sdkmanager 'build-tools;35.0.0'".into(),
                json!({"path":t.build_tools}),
            );
        }
        let signer = t
            .build_tools
            .as_ref()
            .is_some_and(|p| p.join("lib/apksigner.jar").is_file());
        add(
            "ARA-EXP-AND-017",
            signer,
            false,
            "apksigner jar",
            "sdkmanager 'build-tools;35.0.0'".into(),
            json!({}),
        );
        let platform = t
            .sdk
            .as_ref()
            .map(|p| p.join(format!("platforms/android-{}/android.jar", opts.target_api)));
        add(
            "ARA-EXP-AND-004",
            platform.as_ref().is_some_and(|p| p.is_file()) && opts.target_api >= 36,
            false,
            "Play target API platform",
            format!("sdkmanager 'platforms;android-{}'", opts.target_api),
            json!({"target_api":opts.target_api,"jar":platform}),
        );
        let java = t
            .java_tool("java")
            .map(|p| probe(&p, &["-version"], "java"))
            .unwrap_or_default();
        add(
            "ARA-EXP-AND-005",
            tools::version(&java).first().is_some_and(|v| *v >= 17),
            false,
            "Java 17+",
            "winget install Microsoft.OpenJDK.17".into(),
            json!({"version":java.trim()}),
        );
        add(
            "ARA-EXP-AND-006",
            t.bundletool.is_some(),
            false,
            "bundletool for required AAB output",
            "$env:ARA_BUNDLETOOL='<path>/bundletool-all-1.18.3.jar'".into(),
            json!({"path":t.bundletool}),
        );
        add(
            "ARA-EXP-AND-008",
            ["aarch64-linux-android", "x86_64-linux-android"]
                .iter()
                .all(|t| targets.lines().any(|l| l == *t)),
            true,
            "Android runtime template targets",
            "rustup target add aarch64-linux-android x86_64-linux-android".into(),
            json!({}),
        );
        add(
            "ARA-EXP-AND-010",
            cached("microsoft.netcore.app.runtime.nativeaot.linux-bionic-arm64")
                && cached("microsoft.netcore.app.runtime.nativeaot.linux-bionic-x64"),
            true,
            "NativeAOT bionic runtime packs cached",
            "dotnet restore --runtime linux-bionic-arm64 -p:PublishAotUsingRuntimePack=true".into(),
            json!({}),
        );
        add(
            "ARA-EXP-AND-011",
            t.adb().is_some(),
            true,
            "SDK adb for optional device verification",
            "sdkmanager platform-tools".into(),
            json!({"path":t.adb()}),
        );
        add(
            "ARA-EXP-AND-018",
            t.java_tool("keytool").is_some() && t.java_tool("jarsigner").is_some(),
            false,
            "JDK keystore and AAB signing tools",
            "winget install Microsoft.OpenJDK.17".into(),
            json!({}),
        );
        if opts.devices {
            let devices = t.adb().map(|p| ara_bridge::Adb::new(p).list());
            let ready = devices.as_ref().is_some_and(|d| {
                d.as_ref()
                    .is_ok_and(|d| d.iter().any(|d| d.state == "device"))
            });
            add(
                "ARA-EXP-AND-009",
                ready,
                true,
                "Connected authorized device (emulators are never auto-started)",
                "adb devices -l".into(),
                json!({"ready":ready}),
            );
        }
    }
    if opts
        .platform
        .is_some_and(|p| matches!(p, Platform::Macos | Platform::Ios))
    {
        add(
            "ARA-EXP-APL-005",
            tools::which("ssh").is_some(),
            false,
            "SSH client for Apple silicon Mac agent",
            "Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0".into(),
            json!({}),
        );
        add(
            "ARA-EXP-APL-002",
            false,
            true,
            "Mac-agent build/sign/verify requires the Mac session",
            "ssh <mac-agent> ara-export-agent --capabilities".into(),
            json!({"protocol":"ara.export.agent/1","arch":"arm64"}),
        );
    }
    crate::bus::emit_checks(&out);
    out
}

/// Checks a project's actual target API and signing configuration in addition to
/// host tools. Password values are never copied into diagnostics or probe logs.
pub fn doctor_for_preset(
    project: &Path,
    preset: &crate::ExportPreset,
    opts: &DoctorOptions,
    t: &Toolchain,
) -> Vec<Check> {
    let scoped = DoctorOptions {
        platform: Some(preset.platform),
        target_api: preset.target_api,
        devices: opts.devices,
        scratch: opts.scratch.clone(),
    };
    let mut checks = doctor(&scoped, t);
    if preset.platform == Platform::Android {
        let (status, message, context) = if let Some(signing) = &preset.signing {
            let key = tools::project_path(project, &signing.keystore).ok();
            let passwords = std::env::var_os(&signing.store_password_env).is_some()
                && std::env::var_os(&signing.key_password_env).is_some();
            let usable = key
                .as_ref()
                .zip(t.java_tool("keytool"))
                .is_some_and(|(key, tool)| {
                    passwords
                        && tools::run(
                            std::process::Command::new(tool)
                                .args(["-list", "-keystore"])
                                .arg(key)
                                .args([
                                    "-alias",
                                    &signing.alias,
                                    "-storepass:env",
                                    &signing.store_password_env,
                                ]),
                            &opts.scratch.join("keystore.log"),
                            std::time::Duration::from_secs(10),
                        )
                        .is_ok()
                });
            (
                if usable { Status::Ok } else { Status::Error },
                "Configured Android upload key/alias and password environment",
                json!({"keystore":key,"alias":signing.alias,"password_environment_present":passwords}),
            )
        } else {
            (
                Status::Warning,
                "Android export uses a generated debug key, unsuitable for Play upload",
                json!({"store_ready":false}),
            )
        };
        checks.push(Check {
            code: "ARA-EXP-AND-007".into(),
            status,
            message: message.into(),
            context,
            fixes: if status == Status::Ok {
                vec![]
            } else {
                vec![
                    "ara-export schema".into(),
                    "keytool -list -keystore <project upload.jks>".into(),
                ]
            },
        });
    }
    crate::bus::emit_checks(&checks);
    checks
}
