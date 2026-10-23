//! `ara-s7 doctor`: real checks for every prerequisite the export lanes use.
//! Read-only: it never installs anything, and leaves WSL and adb as it found them.

use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, ExitCode};

use serde_json::{Value, json};

use crate::env;
use crate::events::{Events, Fix, Status};

struct Doctor<'a> {
    ev: &'a Events,
    errors: u32,
    warnings: u32,
}

impl Doctor<'_> {
    fn check(
        &mut self,
        id: &str,
        status: Status,
        code: Option<&str>,
        message: &str,
        fixes: &[Fix],
        ctx: Value,
    ) {
        match status {
            Status::Error => self.errors += 1,
            Status::Warn => self.warnings += 1,
            Status::Ok => {}
        }
        self.ev.check(id, status, code, message, fixes, ctx);
    }

    fn ok(&mut self, id: &str, message: &str, ctx: Value) {
        self.check(id, Status::Ok, None, message, &[], ctx);
    }
}

/// Runs the checks for `platform` (all three when `None`) and emits a `result` event.
pub fn run(ev: &Events, platform: Option<&str>, devices: bool) -> ExitCode {
    let mut d = Doctor {
        ev,
        errors: 0,
        warnings: 0,
    };
    common(&mut d);
    let platforms: Vec<&str> = match platform {
        Some(p) => vec![p],
        None => vec!["windows", "linux", "android"],
    };
    for p in &platforms {
        match *p {
            "windows" => windows(&mut d),
            "linux" => linux(&mut d),
            "android" => android(&mut d, devices),
            other => d.check(
                "platform",
                Status::Error,
                Some("ARA-EXP-CLI-002"),
                &format!("unknown platform {other}; this spike covers windows, linux, android"),
                &[Fix::doc(
                    "spikes/s7-export/RESULT.md#proposed-ara-export-contract",
                )],
                json!({}),
            ),
        }
    }
    ev.result(json!({"platforms": platforms, "errors": d.errors, "warnings": d.warnings, "ready": d.errors == 0}));
    if d.errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn common(d: &mut Doctor) {
    let sdks = env::capture("dotnet", &["--list-sdks"]).unwrap_or_default();
    let best = sdks
        .lines()
        .filter_map(env::version_in)
        .filter(|v| v.first() == Some(&env::DOTNET_MAJOR))
        .max();
    match best {
        Some(v) => d.ok(
            "dotnet.sdk",
            &format!(".NET SDK {}", join(&v)),
            json!({"sdks": sdks.lines().collect::<Vec<_>>()}),
        ),
        None => d.check(
            "dotnet.sdk",
            Status::Error,
            Some("ARA-EXP-DOT-001"),
            &format!("no .NET {} SDK on PATH", env::DOTNET_MAJOR),
            &[Fix::cmd("winget install Microsoft.DotNet.SDK.10")],
            json!({"found": sdks}),
        ),
    }
    let toolchains = env::capture("rustup", &["toolchain", "list"]).unwrap_or_default();
    if toolchains.lines().any(|l| l.starts_with(env::RUST_PIN)) {
        d.ok(
            "rust.toolchain",
            &format!("Rust {} installed (D17 pin)", env::RUST_PIN),
            json!({}),
        );
    } else {
        d.check(
            "rust.toolchain",
            Status::Error,
            Some("ARA-EXP-RS-001"),
            &format!("Rust toolchain {} (D17 pin) not installed", env::RUST_PIN),
            &[Fix::cmd(format!(
                "rustup toolchain install {} --profile minimal",
                env::RUST_PIN
            ))],
            json!({"toolchains": toolchains}),
        );
    }
}

fn nuget_has(package: &str) -> Option<String> {
    let home = std::env::var_os("NUGET_PACKAGES")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| Path::new(&h).join(".nuget/packages"))
        })?;
    let dir = home.join(package);
    let mut versions: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|v| v.starts_with(&format!("{}.", env::DOTNET_MAJOR)))
        .collect();
    versions.sort();
    versions.pop()
}

fn windows(d: &mut Doctor) {
    if !cfg!(windows) {
        d.check(
            "windows.host",
            Status::Error,
            Some("ARA-EXP-WIN-000"),
            "Windows NativeAOT needs a Windows host (MSVC link.exe)",
            &[],
            json!({}),
        );
        return;
    }
    let Some(vswhere) = env::vswhere() else {
        d.check(
            "windows.msvc",
            Status::Error,
            Some("ARA-EXP-WIN-001"),
            "Visual Studio Installer (vswhere.exe) not found; NativeAOT needs MSVC link.exe and a Windows SDK",
            &[Fix::cmd("winget install Microsoft.VisualStudio.2022.BuildTools --override \"--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended\"")],
            json!({}),
        );
        return;
    };
    let vs = env::capture(
        &vswhere,
        &[
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ],
    );
    match vs.filter(|s| !s.is_empty()) {
        Some(path) => d.ok(
            "windows.msvc",
            "MSVC C++ build tools present",
            json!({"installationPath": path}),
        ),
        None => d.check(
            "windows.msvc",
            Status::Error,
            Some("ARA-EXP-WIN-001"),
            "no Visual Studio instance with the VC x64 tools; NativeAOT links with MSVC",
            &[Fix::cmd(format!(
                "\"{}\" modify --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended",
                vswhere.with_file_name("setup.exe").display()
            ))],
            json!({}),
        ),
    }
    let on_path = env::which("vswhere").is_some();
    d.ok(
        "windows.vswhere_path",
        if on_path { "vswhere.exe on PATH" } else { "vswhere.exe not on PATH (raw `dotnet publish -p:PublishAot=true` fails with MSB3073); ara export prepends it" },
        json!({"on_path": on_path, "dir": vswhere.parent()}),
    );
    match nuget_has("runtime.win-x64.microsoft.dotnet.ilcompiler") {
        Some(v) => d.ok(
            "windows.ilcompiler",
            &format!("ILCompiler win-x64 {v} cached (offline export ready)"),
            json!({}),
        ),
        None => d.check(
            "windows.ilcompiler",
            Status::Warn,
            Some("ARA-EXP-WIN-003"),
            "ILCompiler win-x64 not cached; the first AOT export downloads it (~50 MB)",
            &[],
            json!({}),
        ),
    }
    let signtool = std::env::var_os("ProgramFiles(x86)")
        .map(|p| Path::new(&p).join("Windows Kits/10/bin"))
        .and_then(|bin| {
            let mut v: Vec<_> = std::fs::read_dir(bin)
                .ok()?
                .filter_map(|e| e.ok().map(|e| e.path().join("x64/signtool.exe")))
                .filter(|p| p.is_file())
                .collect();
            v.sort();
            v.pop()
        });
    let cert = std::env::var("ARA_WIN_SIGN").ok();
    match (&signtool, &cert) {
        (Some(st), Some(c)) => d.ok("windows.signing", &format!("signtool + {c}"), json!({"signtool": st})),
        _ => d.check(
            "windows.signing",
            Status::Warn,
            Some("ARA-EXP-WIN-010"),
            "no Authenticode identity configured; Windows exports will be unsigned (SmartScreen warns on direct downloads; Steam does not require signing)",
            &[
                Fix::env("ARA_WIN_SIGN=azure:<account>/<profile>  (Azure Artifact Signing, ~$10/mo)"),
                Fix::cmd(format!(
                    "\"{}\" sign /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 /dlib Azure.CodeSigning.Dlib.dll /dmdf metadata.json dist\\windows-aot\\ara-player.exe dist\\windows-aot\\game\\Game.dll",
                    signtool.as_deref().map_or("signtool.exe".into(), |p| p.display().to_string())
                )),
                Fix::doc("https://learn.microsoft.com/windows/apps/package-and-deploy/code-signing-options"),
            ],
            json!({"signtool": signtool}),
        ),
    }
}

fn wsl_state() -> Option<String> {
    let out = Command::new("wsl.exe")
        .args(["--list", "--verbose"])
        .output()
        .ok()?;
    let text = env::wsl_text(&out.stdout);
    let distro = env::wsl_distro();
    text.lines()
        .map(|l| {
            l.trim_start_matches(['*', ' '])
                .split_whitespace()
                .collect::<Vec<_>>()
        })
        .find(|cols| cols.first() == Some(&distro.as_str()))
        .and_then(|cols| cols.get(1).map(|s| s.to_string()))
}

fn linux(d: &mut Doctor) {
    let distro = env::wsl_distro();
    let Some(state) = wsl_state() else {
        d.check(
            "linux.wsl",
            Status::Error,
            Some("ARA-EXP-LNX-001"),
            &format!("WSL distro {distro} not found; the NativeAOT Linux lane builds inside it"),
            &[
                Fix::cmd(format!("wsl --install -d {distro}")),
                Fix::env("ARA_WSL_DISTRO=<name>"),
            ],
            json!({}),
        );
        return;
    };
    let was_stopped = state.eq_ignore_ascii_case("stopped");
    d.ok(
        "linux.wsl",
        &format!("WSL {distro} ({state})"),
        json!({"state": state}),
    );

    let probe = r#"printf 'dotnet=%s\n' "$($HOME/.dotnet/dotnet --version 2>/dev/null || dotnet --version 2>/dev/null)";
printf 'clang=%s\n' "$(command -v clang)"; printf 'lld=%s\n' "$(command -v ld.lld)";
printf 'zlib=%s\n' "$(test -f /usr/include/zlib.h && echo yes)";
printf 'rustc=%s\n' "$($HOME/.cargo/bin/rustc --version 2>/dev/null)";
printf 'glibc=%s\n' "$(ldd --version 2>/dev/null | head -1)""#;
    let Some((_, out)) = env::wsl_bash(probe) else {
        d.check(
            "linux.probe",
            Status::Error,
            Some("ARA-EXP-LNX-002"),
            "could not run bash in WSL",
            &[],
            json!({}),
        );
        return;
    };
    if was_stopped {
        let _ = Command::new("wsl.exe")
            .args(["--terminate", &distro])
            .output();
    }
    let get = |k: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let setup = Fix::cmd(format!(
        "wsl -d {distro} -e bash spikes/s7-export/scripts/wsl-setup.sh"
    ));
    let dotnet = get("dotnet");
    if env::version_in(&dotnet).and_then(|v| v.first().copied()) == Some(env::DOTNET_MAJOR) {
        d.ok(
            "linux.dotnet",
            &format!(".NET SDK {dotnet} in WSL"),
            json!({}),
        );
    } else {
        d.check(
            "linux.dotnet",
            Status::Error,
            Some("ARA-EXP-LNX-003"),
            "no .NET 10 SDK in WSL",
            std::slice::from_ref(&setup),
            json!({"found": dotnet}),
        );
    }
    if !get("clang").is_empty() && !get("zlib").is_empty() {
        d.ok(
            "linux.clang",
            "clang + zlib headers in WSL (NativeAOT link prerequisites)",
            json!({"clang": get("clang"), "lld": get("lld")}),
        );
    } else {
        d.check(
            "linux.clang",
            Status::Error,
            Some("ARA-EXP-LNX-004"),
            "NativeAOT on Linux needs clang and zlib1g-dev",
            &[Fix::cmd(format!(
                "wsl -d {distro} -u root -e apt-get install -y clang lld zlib1g-dev"
            ))],
            json!({"clang": get("clang"), "zlib": get("zlib")}),
        );
    }
    let rustc = get("rustc");
    if rustc.contains(env::RUST_PIN) {
        d.ok("linux.rust", &rustc, json!({}));
    } else {
        d.check(
            "linux.rust",
            Status::Error,
            Some("ARA-EXP-LNX-005"),
            &format!("Rust {} not installed in WSL", env::RUST_PIN),
            &[setup],
            json!({"found": rustc}),
        );
    }
    match std::env::var_os("ARA_ZIG").map(std::path::PathBuf::from).filter(|p| p.is_file()).or_else(|| {
        std::fs::read_dir(env::root().join("tools")).ok()?.filter_map(|e| e.ok().map(|e| e.path().join("zig.exe"))).find(|p| p.is_file())
    }) {
        Some(zig) => d.ok("linux.zig", "zig present: aot-cross and r2r lanes build on Windows against glibc 2.31", json!({"zig": zig})),
        None => d.check(
            "linux.zig",
            Status::Error,
            Some("ARA-EXP-LNX-006"),
            "zig not found; the Windows-hosted Linux lanes (aot-cross, r2r) link with zigcc",
            &[Fix::cmd("curl -L -o zig.zip https://ziglang.org/download/0.17.0/zig-x86_64-windows-0.17.0.zip && unzip zig.zip -d spikes/s7-export/tools"), Fix::env("ARA_ZIG=<path to zig.exe>")],
            json!({}),
        ),
    }
    let glibc = get("glibc");
    let v = glibc
        .rsplit(' ')
        .next()
        .and_then(env::version_in)
        .unwrap_or_default();
    let (maj, min) = env::GLIBC_BASELINE;
    if v.len() >= 2 && (v[0], v[1]) <= (maj, min) {
        d.ok(
            "linux.glibc",
            &format!("build glibc {} <= {maj}.{min} baseline", join(&v)),
            json!({}),
        );
    } else {
        d.check(
            "linux.glibc",
            Status::Warn,
            Some("ARA-EXP-LNX-010"),
            &format!("the WSL lane (`--lane aot`) links against glibc {}; Steam Runtime 3 (sniper) has {maj}.{min}, so its binaries do not start there (aot-cross and r2r are pinned to 2.31)", join(&v)),
            &[
                Fix::doc("Build Linux templates in the sniper SDK container: registry.gitlab.steamos.cloud/steamrt/sniper/sdk"),
                Fix::doc("Or link against a glibc 2.31 sysroot (zig cc -target x86_64-linux-gnu.2.31)"),
            ],
            json!({"glibc": glibc}),
        );
    }
    match nuget_has("microsoft.netcore.app.runtime.linux-x64") {
        Some(v) => d.ok(
            "linux.r2r_cross",
            &format!("linux-x64 runtime pack {v} cached: R2R exports to Linux run on Windows"),
            json!({}),
        ),
        None => d.check(
            "linux.r2r_cross",
            Status::Warn,
            Some("ARA-EXP-LNX-011"),
            "linux-x64 runtime pack not cached; the first R2R Linux export downloads it",
            &[],
            json!({}),
        ),
    }
}

fn android(d: &mut Doctor, devices: bool) {
    let Some(sdk) = env::android_sdk() else {
        d.check(
            "android.sdk",
            Status::Error,
            Some("ARA-EXP-AND-001"),
            "Android SDK not found (ANDROID_HOME / ANDROID_SDK_ROOT / %LOCALAPPDATA%\\Android\\Sdk)",
            &[Fix::env("ANDROID_HOME=<sdk path>")],
            json!({}),
        );
        return;
    };
    d.ok(
        "android.sdk",
        &format!("SDK at {}", sdk.display()),
        json!({}),
    );
    let sdkmanager = sdk.join("cmdline-tools/latest/bin/sdkmanager.bat");
    let sm = |pkg: &str| Fix::cmd(format!("\"{}\" \"{pkg}\"", sdkmanager.display()));
    match env::ndk(Some(&sdk)) {
        Some((dir, rev))
            if env::version_in(&rev)
                .and_then(|v| v.first().copied())
                .unwrap_or(0)
                >= env::NDK_MIN_MAJOR =>
        {
            let clang = env::ndk_bin(&dir).join(if cfg!(windows) { "clang.exe" } else { "clang" });
            if clang.is_file() {
                d.ok("android.ndk", &format!("NDK {rev}"), json!({"path": dir}));
            } else {
                d.check(
                    "android.ndk",
                    Status::Error,
                    Some("ARA-EXP-AND-002"),
                    "NDK has no host clang",
                    &[sm("ndk;28.2.13676358")],
                    json!({"path": dir}),
                );
            }
        }
        found => d.check(
            "android.ndk",
            Status::Error,
            Some("ARA-EXP-AND-002"),
            &format!(
                "NDK r{}+ required (NativeAOT bionic link + Rust template)",
                env::NDK_MIN_MAJOR
            ),
            &[sm("ndk;28.2.13676358")],
            json!({"found": found.map(|f| f.1)}),
        ),
    }
    let bt = env::build_tools(&sdk);
    let tools_ok = bt.as_ref().is_some_and(|b| {
        ["aapt2", "zipalign"].iter().all(|t| {
            b.join(format!("{t}{}", std::env::consts::EXE_SUFFIX))
                .is_file()
        }) && b
            .join(if cfg!(windows) {
                "apksigner.bat"
            } else {
                "apksigner"
            })
            .is_file()
    });
    if tools_ok {
        d.ok(
            "android.build_tools",
            "aapt2, zipalign, apksigner present",
            json!({"path": bt}),
        );
    } else {
        d.check(
            "android.build_tools",
            Status::Error,
            Some("ARA-EXP-AND-003"),
            "build-tools with aapt2/zipalign/apksigner missing",
            &[sm("build-tools;36.0.0")],
            json!({"path": bt}),
        );
    }
    match env::platform_jar(&sdk) {
        Some(jar) => d.ok(
            "android.platform",
            &format!("android-{} platform", env::ANDROID_TARGET_API),
            json!({"jar": jar}),
        ),
        None => d.check(
            "android.platform",
            Status::Error,
            Some("ARA-EXP-AND-004"),
            &format!(
                "platforms;android-{} missing (Play requires target API {})",
                env::ANDROID_TARGET_API,
                env::ANDROID_TARGET_API
            ),
            &[sm(&format!(
                "platforms;android-{}",
                env::ANDROID_TARGET_API
            ))],
            json!({}),
        ),
    }
    let java = env::jdk_tool("java").and_then(|j| env::capture(j, &["-version"]));
    let major = java
        .as_deref()
        .and_then(env::version_in)
        .and_then(|v| v.first().copied())
        .unwrap_or(0);
    if major >= 17 {
        d.ok(
            "android.java",
            &format!("Java {major} (apksigner, bundletool)"),
            json!({}),
        );
    } else {
        d.check(
            "android.java",
            Status::Error,
            Some("ARA-EXP-AND-005"),
            "JDK 17+ required for apksigner and bundletool",
            &[
                Fix::cmd("winget install Microsoft.OpenJDK.17"),
                Fix::env("JAVA_HOME=<jdk>"),
            ],
            json!({"found": java}),
        );
    }
    match env::bundletool() {
        Some(p) => d.ok("android.bundletool", "bundletool present (AAB lane)", json!({"path": p})),
        None => d.check(
            "android.bundletool",
            Status::Warn,
            Some("ARA-EXP-AND-006"),
            "bundletool not found; APK still builds, AAB is skipped",
            &[Fix::cmd("curl -L -o spikes/s7-export/tools/bundletool.jar https://github.com/google/bundletool/releases/download/1.18.3/bundletool-all-1.18.3.jar")],
            json!({}),
        ),
    }
    if std::env::var_os("ARA_ANDROID_KEYSTORE").is_some() {
        d.ok("android.signing", "release keystore configured", json!({}));
    } else {
        d.check(
            "android.signing",
            Status::Warn,
            Some("ARA-EXP-AND-007"),
            "no upload keystore; exports are signed with a generated debug key (fine for sideload, not for Play)",
            &[Fix::env("ARA_ANDROID_KEYSTORE=<path.jks>;ARA_ANDROID_KEY_ALIAS=<alias>")],
            json!({}),
        );
    }
    let targets = env::capture(
        "rustup",
        &[
            "target",
            "list",
            "--installed",
            "--toolchain",
            env::RUST_PIN,
        ],
    )
    .unwrap_or_default();
    let missing: Vec<&str> = ["aarch64-linux-android", "x86_64-linux-android"]
        .into_iter()
        .filter(|t| !targets.lines().any(|l| l == *t))
        .collect();
    if missing.is_empty() {
        d.ok(
            "android.rust_targets",
            "aarch64-linux-android, x86_64-linux-android",
            json!({}),
        );
    } else {
        d.check(
            "android.rust_targets",
            Status::Error,
            Some("ARA-EXP-AND-008"),
            &format!("missing Rust targets: {}", missing.join(", ")),
            &[Fix::cmd(format!(
                "rustup target add --toolchain {} {}",
                env::RUST_PIN,
                missing.join(" ")
            ))],
            json!({}),
        );
    }
    // PublishAotUsingRuntimePack=true pulls the NativeAOT runtime pack, not the ILCompiler RID package.
    match nuget_has("microsoft.netcore.app.runtime.nativeaot.linux-bionic-arm64") {
        Some(v) => d.ok("android.ilcompiler", &format!("NativeAOT runtime pack linux-bionic-arm64 {v} cached"), json!({})),
        None => d.check("android.ilcompiler", Status::Warn, Some("ARA-EXP-AND-010"), "NativeAOT linux-bionic-arm64 runtime pack not cached; the first Android export downloads it", &[], json!({})),
    }
    // Two adb versions (SDK vs PATH) kill each other's server; installs then fail mid-stream.
    if let (Some(sdk_adb), Some(path_adb)) = (env::adb(Some(&sdk)), env::which("adb")) {
        let sdk_v = env::capture(&sdk_adb, &["version"])
            .and_then(|t| t.lines().nth(1).map(String::from))
            .unwrap_or_default();
        let path_v = env::capture(&path_adb, &["version"])
            .and_then(|t| t.lines().nth(1).map(String::from))
            .unwrap_or_default();
        if sdk_v == path_v {
            d.ok("android.adb", &sdk_v, json!({"path": sdk_adb}));
        } else {
            d.check(
                "android.adb",
                Status::Warn,
                Some("ARA-EXP-AND-012"),
                &format!("adb on PATH ({path_v}) differs from the SDK's ({sdk_v}); the emulator and tools restart each other's server"),
                &[Fix::env(format!("PATH=<put {} first>", sdk_adb.parent().map(|p| p.display().to_string()).unwrap_or_default()))],
                json!({"sdk": sdk_adb, "path": path_adb}),
            );
        }
    }
    if devices {
        android_devices(d, &sdk);
    }
}

fn android_devices(d: &mut Doctor, sdk: &Path) {
    let Some(adb) = env::adb(Some(sdk)) else {
        d.check(
            "android.adb",
            Status::Error,
            Some("ARA-EXP-AND-011"),
            "adb not found",
            &[Fix::cmd("sdkmanager platform-tools")],
            json!({}),
        );
        return;
    };
    let server_was_up = TcpStream::connect("127.0.0.1:5037").is_ok();
    let list = env::capture(&adb, &["devices"]).unwrap_or_default();
    if !server_was_up {
        let _ = Command::new(&adb).arg("kill-server").output();
    }
    let devices: Vec<&str> = list
        .lines()
        .skip(1)
        .filter(|l| l.ends_with("device"))
        .collect();
    if !devices.is_empty() {
        d.ok(
            "android.device",
            &format!("{} device(s): {}", devices.len(), devices.join(", ")),
            json!({}),
        );
        return;
    }
    let avds = env::capture(sdk.join("emulator/emulator"), &["-list-avds"]).unwrap_or_default();
    let avd = avds
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("INFO"));
    let mut fixes = vec![Fix::doc(
        "Connect a phone with USB debugging, or start an emulator",
    )];
    if let Some(avd) = avd {
        fixes.insert(
            0,
            Fix::cmd(format!(
                "\"{}\" -avd {avd} -no-window -no-audio -no-snapshot",
                sdk.join("emulator/emulator").display()
            )),
        );
    }
    d.check(
        "android.device",
        Status::Warn,
        Some("ARA-EXP-AND-009"),
        "no device attached; the verify stage will be skipped",
        &fixes,
        json!({"avds": avds.lines().collect::<Vec<_>>()}),
    );
}

fn join(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
}
