//! `ara-s7 export`: the template lane for Windows, Linux and Android.
//!
//! Every lane runs the same stages: `template` (in Ara a hash-pinned download; built
//! here), `script` (C# → NativeAOT image or R2R folder), `pack`, `assemble`/`package`,
//! `sign`, `verify`. Each stage emits `progress` then `stage` (with ms), or a coded
//! `error` with fixes. Tool output goes to `out/logs/<lane>-<stage>.log`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::env;
use crate::events::{Events, Fix};

/// Parsed `export` flags.
pub struct Options {
    /// `windows`, `linux` or `android`.
    pub platform: String,
    /// `aot` (default) or `r2r`.
    pub lane: Option<String>,
    /// Only `release` is meaningful in the spike.
    pub profile: String,
    /// Remove C# obj/bin and this lane's out/dist first (cold export).
    pub clean: bool,
    /// Run the exported build and check its result line.
    pub verify: bool,
}

/// Marks an error already reported as an `error` event.
#[derive(Debug)]
pub struct Reported;

impl std::fmt::Display for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("export failed (see error event)")
    }
}

impl std::error::Error for Reported {}

struct Lane<'a> {
    ev: &'a Events,
    name: String,
    root: PathBuf,
    out: PathBuf,
    dist: PathBuf,
    stages: Vec<Value>,
}

impl Lane<'_> {
    fn new<'a>(ev: &'a Events, name: &str, clean: bool) -> Result<Lane<'a>> {
        let root = env::root();
        let out = root.join("out").join(name);
        let dist = root.join("dist").join(name);
        if clean {
            for dir in ["game/bin", "game/obj", "mod/bin", "mod/obj"] {
                remove(&root.join(dir))?;
            }
            remove(&out)?;
            remove(&dist)?;
        }
        std::fs::create_dir_all(root.join("out/logs"))?;
        std::fs::create_dir_all(&out)?;
        Ok(Lane {
            ev,
            name: name.into(),
            root,
            out,
            dist,
            stages: Vec::new(),
        })
    }

    fn log(&self, stage: &str) -> PathBuf {
        self.root
            .join("out/logs")
            .join(format!("{}-{stage}.log", self.name))
    }

    /// Runs one stage, timing it and turning failures into a coded `error` event.
    fn stage<T>(
        &mut self,
        stage: &str,
        code: &str,
        fixes: &[Fix],
        f: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        self.ev.progress(stage, "started");
        let started = Instant::now();
        match f(self) {
            Ok(v) => {
                let ms = ms(started.elapsed());
                self.ev.stage(stage, ms, Value::Null);
                self.stages.push(json!({"stage": stage, "ms": ms}));
                Ok(v)
            }
            Err(e) => {
                self.ev.error(
                    code,
                    stage,
                    &format!("{e:#}"),
                    fixes,
                    json!({"lane": self.name, "log": self.log(stage)}),
                );
                Err(anyhow::Error::new(Reported))
            }
        }
    }
}

/// Dispatches to the platform lane and emits the final `result` event.
pub fn run(ev: &Events, opts: &Options) -> Result<ExitCode> {
    if opts.profile != "release" {
        ev.warning(
            "ARA-EXP-CLI-003",
            "resolve",
            &format!("profile {} treated as release in the spike", opts.profile),
            &[],
            json!({}),
        );
    }
    let lane = opts.lane.clone().unwrap_or_else(|| "aot".into());
    let started = Instant::now();
    let result = match (opts.platform.as_str(), lane.as_str()) {
        ("windows", "aot" | "r2r") => windows(ev, opts, &lane),
        ("linux", "aot" | "aot-cross" | "r2r") => linux(ev, opts, &lane),
        ("android", "aot") => android(ev, opts),
        (p, l) => {
            ev.error(
                "ARA-EXP-CLI-002",
                "resolve",
                &format!("no lane {l} for platform {p}"),
                &[Fix::doc(
                    "windows: aot|r2r, linux: aot|aot-cross|r2r, android: aot",
                )],
                json!({}),
            );
            return Ok(ExitCode::from(2));
        }
    };
    match result {
        Ok(mut summary) => {
            summary["total_ms"] = json!(ms(started.elapsed()));
            summary["clean"] = json!(opts.clean);
            let ok = summary["verify"]["pass"].as_bool().unwrap_or(true);
            ev.result(summary);
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Err(e) if e.is::<Reported>() => Ok(ExitCode::from(1)),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------- Windows

fn windows(ev: &Events, opts: &Options, lane: &str) -> Result<Value> {
    let mut l = Lane::new(ev, &format!("windows-{lane}"), opts.clean)?;
    let path = path_with(env::vswhere().as_deref().and_then(Path::parent));

    let template = l.stage("template", "ARA-EXP-WIN-101", &[], |l| {
        exec(
            cargo(&l.root).args(["build", "--release", "--bin", "ara-player"]),
            &l.log("template"),
        )?;
        Ok(l.root.join("target/release/ara-player.exe"))
    })?;

    let script = l.out.join("script");
    let fixes = [Fix::cmd("ara-s7 doctor --platform windows")];
    l.stage("script", "ARA-EXP-WIN-102", &fixes, |l| {
        exec(
            dotnet_publish(&l.root, "win-x64", lane, &script).env("PATH", &path),
            &l.log("script"),
        )?;
        build_mod(l)
    })?;

    let pack = l.stage("pack", "ARA-EXP-PACK-001", &[], |l| {
        build_pack(l, "desktop")
    })?;

    l.stage("assemble", "ARA-EXP-WIN-104", &[], |l| {
        reset(&l.dist)?;
        copy(&template, &l.dist.join("ara-player.exe"))?;
        assemble_script(l, lane, &script, "Game.dll")?;
        copy(&pack, &l.dist.join("game.arapack"))?;
        copy(&l.out.join("mod/Mod.dll"), &l.dist.join("mods/Mod.dll"))
    })?;

    sign_windows(&l);

    let verify = if opts.verify {
        let exe = l.dist.join("ara-player.exe");
        let report = l.stage("verify", "ARA-EXP-WIN-106", &[], |l| {
            run_local(&exe, &l.log("verify"))
        })?;
        check_report(ev, &report, lane)
    } else {
        Value::Null
    };
    Ok(summary(&l, lane, "game", verify))
}

/// The C# part of a desktop export: one NativeAOT image, or the CoreCLR folder.
fn assemble_script(l: &Lane, lane: &str, script: &Path, aot_name: &str) -> Result<()> {
    let launch = if lane != "r2r" {
        copy(&script.join(aot_name), &l.dist.join("game").join(aot_name))?;
        json!({"pack": "game.arapack", "lane": "nativeaot", "script": format!("game/{aot_name}")})
    } else {
        // Drop symbols and the SDK's apphost (Game.exe / Game): ara-player hosts CoreCLR itself.
        let keep = |p: &Path| {
            let apphost = p.file_stem().is_some_and(|s| s == "Game")
                && p.extension().is_none_or(|e| e == "exe");
            !apphost && !p.extension().is_some_and(|e| e == "pdb" || e == "dbg")
        };
        copy_dir(script, &l.dist.join("dotnet"), &keep)?;
        json!({"pack": "game.arapack", "lane": "r2r-coreclr", "script": "dotnet", "assembly": "Game"})
    };
    std::fs::write(l.dist.join("ara-launch.json"), serde_json::to_vec(&launch)?)?;
    Ok(())
}

fn sign_windows(l: &Lane) {
    match std::env::var("ARA_WIN_SIGN") {
        Ok(identity) => l.ev.warning(
            "ARA-EXP-WIN-011",
            "sign",
            &format!("signing identity {identity} configured, but the spike only documents signing"),
            &[],
            json!({}),
        ),
        Err(_) => l.ev.warning(
            "ARA-EXP-WIN-010",
            "sign",
            "unsigned: no Authenticode identity (ARA_WIN_SIGN). Sign ara-player.exe and every native/managed DLL",
            &[
                Fix::cmd(format!(
                    "signtool sign /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 /dlib Azure.CodeSigning.Dlib.dll /dmdf metadata.json {}",
                    l.dist.join("ara-player.exe").display()
                )),
                Fix::doc("RESULT.md#windows-signing"),
            ],
            json!({"dist": l.dist}),
        ),
    }
}

// ---------------------------------------------------------------- Linux

/// Linux lanes. `aot`: template and NativeAOT image built inside WSL (glibc of the distro).
/// `aot-cross` and `r2r`: everything built on Windows; the template and the NativeAOT image
/// are linked by `zigcc` against glibc 2.31. WSL is only the "device" for audit and verify.
fn linux(ev: &Events, opts: &Options, lane: &str) -> Result<Value> {
    let mut l = Lane::new(ev, &format!("linux-{lane}"), opts.clean)?;
    let win_root = env::wsl_path(&l.root);
    let wsl_fix = [Fix::cmd("ara-s7 doctor --platform linux")];
    let zig_fix = [
        Fix::cmd("ara-s7 doctor --platform linux"),
        Fix::doc("unpack zig into spikes/s7-export/tools/ or set ARA_ZIG"),
    ];
    let zigcc = l.root.join(format!(
        "target/release/zigcc{}",
        std::env::consts::EXE_SUFFIX
    ));
    let ndk_bin = env::ndk(env::android_sdk().as_deref()).map(|(dir, _)| env::ndk_bin(&dir));

    // The template is a CI artifact in Ara; built here so the lane is self-contained.
    let template = if lane == "aot" {
        let templates = l.root.join("out/templates/linux-x64-wsl");
        l.stage("template", "ARA-EXP-LNX-101", &wsl_fix, |l| {
            std::fs::create_dir_all(&templates)?;
            wsl(
                &format!(
                    "mkdir -p ~/ara-s7 && cd '{win_root}' && tar cf - --exclude=./target --exclude=./out --exclude=./dist --exclude=./tools --exclude=./results --exclude=./game/bin --exclude=./game/obj --exclude=./mod/bin --exclude=./mod/obj . | (cd ~/ara-s7 && tar xf -)                      && cd ~/ara-s7 && export CARGO_BUILD_JOBS=4 && ~/.cargo/bin/cargo build --release --bin ara-player && ~/.cargo/bin/cargo build --release --features tools --bin ara-s7                      && cp target/release/ara-player target/release/ara-s7 '{}/'",
                    env::wsl_path(&templates)
                ),
                &l.log("template"),
            )
        })?;
        templates.join("ara-player")
    } else {
        l.stage("template", "ARA-EXP-LNX-101", &zig_fix, |l| {
            exec(
                cargo(&l.root)
                    .args([
                        "build",
                        "--release",
                        "--bin",
                        "ara-player",
                        "--target",
                        "x86_64-unknown-linux-gnu",
                    ])
                    .env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER", &zigcc),
                &l.log("template"),
            )?;
            Ok(l.root
                .join("target/x86_64-unknown-linux-gnu/release/ara-player"))
        })?
    };

    let script = l.out.join("script");
    l.stage("script", "ARA-EXP-LNX-102", if lane == "aot-cross" { &zig_fix } else { &wsl_fix }, |l| {
        match lane {
            "aot" => {
                // NativeAOT with the distro's clang: build inside WSL, copy the image back.
                std::fs::create_dir_all(&script)?;
                wsl(
                    &format!(
                        "cd ~/ara-s7 && ~/.dotnet/dotnet publish game/Game.csproj -c Release -r linux-x64 -p:AraLane=aot -o ~/ara-s7/out/{name}/script --nologo --disable-build-servers                          && cp ~/ara-s7/out/{name}/script/Game.so '{}/'",
                        env::wsl_path(&script),
                        name = l.name
                    ),
                    &l.log("script"),
                )?;
            }
            "aot-cross" => {
                // ILC runs on Windows; zigcc links against glibc 2.31; NDK llvm-objcopy strips.
                let mut cmd = dotnet_publish(&l.root, "linux-x64", "aot", &script);
                cmd.args(["-p:DisableUnsupportedError=true", "-p:CppCompilerAndLinker=zigcc", "-p:LinkerFlavor=lld"]);
                match &ndk_bin {
                    Some(_) => cmd.arg("-p:ObjCopyName=llvm-objcopy"),
                    None => cmd.arg("-p:StripSymbols=false"),
                };
                let mut path = vec![zigcc.parent().context("zigcc has no dir")?.to_path_buf()];
                path.extend(ndk_bin.clone());
                path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
                exec(cmd.env("PATH", std::env::join_paths(path)?), &l.log("script"))?;
            }
            _ => {
                // ReadyToRun cross-compiles from Windows: crossgen2 targets linux-x64 directly.
                exec(&mut dotnet_publish(&l.root, "linux-x64", "r2r", &script), &l.log("script"))?;
            }
        }
        build_mod(l)
    })?;

    let pack = l.stage("pack", "ARA-EXP-PACK-001", &[], |l| {
        build_pack(l, "desktop")
    })?;

    l.stage("assemble", "ARA-EXP-LNX-104", &[], |l| {
        reset(&l.dist)?;
        copy(&template, &l.dist.join("ara-player"))?;
        assemble_script(l, lane, &script, "Game.so")?;
        copy(&pack, &l.dist.join("game.arapack"))?;
        copy(&l.out.join("mod/Mod.dll"), &l.dist.join("mods/Mod.dll"))
    })?;

    let wsl_dist = format!("~/ara-s7/dist/{}", l.name);
    let glibc = l.stage("audit", "ARA-EXP-LNX-105", &[], |l| {
        let out = wsl(
            &format!(
                "rm -rf {wsl_dist} && mkdir -p ~/ara-s7/dist && cp -r '{}' ~/ara-s7/dist/ && cd {wsl_dist} && chmod +x ara-player \
                 && for f in ara-player $(find game dotnet -name '*.so' 2>/dev/null); do printf '%s %s\\n' \"$f\" \"$(objdump -T \"$f\" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)\"; done",
                env::wsl_path(&l.dist)
            ),
            &l.log("audit"),
        )?;
        Ok(out.lines().filter_map(|line| line.split_once(' ')).map(|(f, v)| (f.to_string(), v.trim().to_string())).collect::<Vec<_>>())
    })?;
    let worst = glibc
        .iter()
        .filter_map(|(_, v)| env::version_in(v))
        .max()
        .unwrap_or_default();
    let (maj, min) = env::GLIBC_BASELINE;
    if worst.len() >= 2 && (worst[0], worst[1]) > (maj, min) {
        let offenders: Vec<_> = glibc
            .iter()
            .filter(|(_, v)| {
                env::version_in(v).is_some_and(|x| x.len() >= 2 && (x[0], x[1]) > (maj, min))
            })
            .collect();
        ev.warning(
            "ARA-EXP-LNX-010",
            "audit",
            &format!("needs glibc {}.{}; Steam Runtime 3 (sniper) provides {maj}.{min}", worst[0], worst[1]),
            &[Fix::doc("Build Linux templates (and NativeAOT images) against a glibc 2.31 sysroot: sniper SDK container, or zig cc -target x86_64-linux-gnu.2.31")],
            json!({"files": offenders}),
        );
    }

    let verify = if opts.verify {
        let report = l.stage("verify", "ARA-EXP-LNX-106", &wsl_fix, |l| {
            let out = wsl(
                &format!("cd {wsl_dist} && ./ara-player --mod mods/Mod.dll"),
                &l.log("verify"),
            )?;
            parse_report(&out)
        })?;
        check_report(ev, &report, lane)
    } else {
        Value::Null
    };
    let mut s = summary(&l, lane, "game", verify);
    s["glibc"] = json!(
        glibc
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
    );
    s["built_on"] = json!(if lane == "aot" { "wsl" } else { "windows" });
    Ok(s)
}

// ---------------------------------------------------------------- Android

/// (APK ABI dir, Rust target, .NET RID).
const ANDROID_ABIS: [(&str, &str, &str); 2] = [
    ("arm64-v8a", "aarch64-linux-android", "linux-bionic-arm64"),
    ("x86_64", "x86_64-linux-android", "linux-bionic-x64"),
];
const PACKAGE: &str = "dev.ara.s7";

fn android(ev: &Events, opts: &Options) -> Result<Value> {
    let mut l = Lane::new(ev, "android-aot", opts.clean)?;
    let doctor = [Fix::cmd("ara-s7 doctor --platform android")];
    let sdk = env::android_sdk().context("Android SDK not found (ARA-EXP-AND-001)")?;
    let (ndk, ndk_rev) = env::ndk(Some(&sdk)).context("NDK not found (ARA-EXP-AND-002)")?;
    let ndk_bin = env::ndk_bin(&ndk);
    let bt = env::build_tools(&sdk).context("build-tools not found (ARA-EXP-AND-003)")?;
    let jar = env::platform_jar(&sdk).context("platform jar not found (ARA-EXP-AND-004)")?;
    let jdk =
        |t: &str| env::jdk_tool(t).with_context(|| format!("{t} not found (ARA-EXP-AND-005)"));
    let (java, jar_tool, keytool, jarsigner) = (
        jdk("java")?,
        jdk("jar")?,
        jdk("keytool")?,
        jdk("jarsigner")?,
    );
    let exe = std::env::consts::EXE_SUFFIX;
    let apksigner = bt.join(if cfg!(windows) {
        "apksigner.bat"
    } else {
        "apksigner"
    });

    l.stage("template", "ARA-EXP-AND-101", &doctor, |l| {
        for (_, triple, _) in ANDROID_ABIS {
            let key = triple.to_uppercase().replace('-', "_");
            exec(
                cargo(&l.root)
                    .args(["build", "--release", "--lib", "--target", triple])
                    .env(format!("CARGO_TARGET_{key}_LINKER"), ndk_bin.join(format!("clang{exe}")))
                    .env(
                        format!("CARGO_TARGET_{key}_RUSTFLAGS"),
                        format!("-C link-arg=--target={triple}{} -C link-arg=-Wl,-z,max-page-size=16384", env::ANDROID_MIN_API),
                    ),
                &l.log("template"),
            )?;
        }
        Ok(())
    })?;

    let path = path_with(Some(&ndk_bin));
    l.stage("script", "ARA-EXP-AND-102", &doctor, |l| {
        for (abi, _, rid) in ANDROID_ABIS {
            let dir = l.out.join("script").join(abi);
            // NativeAOT refuses cross-OS by default; the NDK clang on PATH is the bionic linker.
            exec(
                dotnet_publish(&l.root, rid, "aot", &dir)
                    .args([
                        "-p:DisableUnsupportedError=true",
                        "-p:PublishAotUsingRuntimePack=true",
                    ])
                    .env("PATH", &path),
                &l.log(&format!("script-{abi}")),
            )?;
        }
        Ok(())
    })?;

    let pack = l.stage("pack", "ARA-EXP-PACK-001", &[], |l| build_pack(l, "mobile"))?;

    let keystore = l.root.join("out/debug.keystore");
    let ks_args = [
        "--ks",
        &keystore.to_string_lossy(),
        "--ks-pass",
        "pass:android",
        "--key-pass",
        "pass:android",
        "--ks-key-alias",
        "androiddebugkey",
    ]
    .map(String::from);
    let apk = l.dist.join("ara-s7.apk");
    l.stage("package-apk", "ARA-EXP-AND-104", &doctor, |l| {
        reset(&l.dist)?;
        if !keystore.is_file() {
            exec(
                Command::new(&keytool)
                    .args(["-genkeypair", "-keystore"])
                    .arg(&keystore)
                    .args([
                        "-storepass",
                        "android",
                        "-keypass",
                        "android",
                        "-alias",
                        "androiddebugkey",
                        "-keyalg",
                        "RSA",
                        "-keysize",
                        "2048",
                        "-validity",
                        "10000",
                        "-dname",
                        "CN=Android Debug,O=Android,C=US",
                    ]),
                &l.log("keystore"),
            )?;
        }
        let stage_dir = l.out.join("apk");
        reset(&stage_dir)?;
        aapt2_link(
            &bt,
            &jar,
            &l.root,
            &l.out.join("base-res.apk"),
            false,
            &l.log("package-apk"),
        )?;
        exec(
            Command::new(&jar_tool)
                .arg("xf")
                .arg(l.out.join("base-res.apk"))
                .current_dir(&stage_dir),
            &l.log("package-apk"),
        )?;
        add_payload(l, &stage_dir, &pack)?;
        let unaligned = l.out.join("unaligned.apk");
        let _ = std::fs::remove_file(&unaligned);
        // Stored (0): native libs must be uncompressed for extractNativeLibs=false, the pack for AAsset mmap.
        exec(
            Command::new(&jar_tool)
                .arg("cfM0")
                .arg(&unaligned)
                .arg("-C")
                .arg(&stage_dir)
                .arg("."),
            &l.log("package-apk"),
        )?;
        let aligned = l.out.join("aligned.apk");
        exec(
            Command::new(bt.join(format!("zipalign{exe}")))
                .args(["-P", "16", "-f", "4"])
                .arg(&unaligned)
                .arg(&aligned),
            &l.log("package-apk"),
        )?;
        exec(
            Command::new(&apksigner)
                .arg("sign")
                .args(&ks_args)
                .arg("--out")
                .arg(&apk)
                .arg(&aligned),
            &l.log("package-apk"),
        )?;
        exec(
            Command::new(&apksigner)
                .args([
                    "verify",
                    "--min-sdk-version",
                    &env::ANDROID_MIN_API.to_string(),
                ])
                .arg(&apk),
            &l.log("package-apk"),
        )?;
        exec(
            Command::new(bt.join(format!("zipalign{exe}")))
                .args(["-c", "-P", "16", "4"])
                .arg(&apk),
            &l.log("package-apk"),
        )?;
        Ok(())
    })?;

    let aab = l.dist.join("ara-s7.aab");
    let mut device_sizes = Value::Null;
    match env::bundletool() {
        None => ev.warning(
            "ARA-EXP-AND-006",
            "package-aab",
            "bundletool not found; AAB skipped",
            &[Fix::cmd("ara-s7 doctor --platform android")],
            json!({}),
        ),
        Some(bundletool) => {
            device_sizes = l.stage("package-aab", "ARA-EXP-AND-105", &doctor, |l| {
                let module = l.out.join("module");
                reset(&module)?;
                aapt2_link(
                    &bt,
                    &jar,
                    &l.root,
                    &l.out.join("base-proto.apk"),
                    true,
                    &l.log("package-aab"),
                )?;
                exec(
                    Command::new(&jar_tool)
                        .arg("xf")
                        .arg(l.out.join("base-proto.apk"))
                        .current_dir(&module),
                    &l.log("package-aab"),
                )?;
                std::fs::create_dir_all(module.join("manifest"))?;
                std::fs::rename(
                    module.join("AndroidManifest.xml"),
                    module.join("manifest/AndroidManifest.xml"),
                )?;
                add_payload(l, &module, &pack)?;
                let base = l.out.join("base.zip");
                let _ = std::fs::remove_file(&base);
                exec(
                    Command::new(&jar_tool)
                        .arg("cfM")
                        .arg(&base)
                        .arg("-C")
                        .arg(&module)
                        .arg("."),
                    &l.log("package-aab"),
                )?;
                let _ = std::fs::remove_file(&aab);
                exec(
                    Command::new(&java)
                        .arg("-jar")
                        .arg(&bundletool)
                        .arg("build-bundle")
                        .arg(format!("--modules={}", base.display()))
                        .arg(format!("--output={}", aab.display()))
                        .arg(format!(
                            "--config={}",
                            l.root.join("android/BundleConfig.json").display()
                        )),
                    &l.log("package-aab"),
                )?;
                exec(
                    Command::new(&jarsigner)
                        .args(["-keystore"])
                        .arg(&keystore)
                        .args([
                            "-storepass",
                            "android",
                            "-keypass",
                            "android",
                            "-sigalg",
                            "SHA256withRSA",
                            "-digestalg",
                            "SHA-256",
                        ])
                        .arg(&aab)
                        .arg("androiddebugkey"),
                    &l.log("package-aab"),
                )?;
                let apks = l.out.join("ara-s7.apks");
                let _ = std::fs::remove_file(&apks);
                exec(
                    Command::new(&java)
                        .arg("-jar")
                        .arg(&bundletool)
                        .arg("build-apks")
                        .arg(format!("--bundle={}", aab.display()))
                        .arg(format!("--output={}", apks.display()))
                        .arg(format!("--ks={}", keystore.display()))
                        .args([
                            "--ks-pass=pass:android",
                            "--key-pass=pass:android",
                            "--ks-key-alias=androiddebugkey",
                        ]),
                    &l.log("package-aab"),
                )?;
                let sizes = exec(
                    Command::new(&java)
                        .arg("-jar")
                        .arg(&bundletool)
                        .arg("get-size")
                        .arg("total")
                        .arg(format!("--apks={}", apks.display()))
                        .arg("--dimensions=ABI"),
                    &l.log("package-aab-size"),
                )?;
                Ok(json!(
                    sizes
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .collect::<Vec<_>>()
                ))
            })?;
        }
    }

    let verify = if opts.verify {
        android_verify(&mut l, &sdk, &apk)?
    } else {
        Value::Null
    };

    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let per_abi: Vec<Value> = ANDROID_ABIS
        .iter()
        .map(|(abi, triple, _)| {
            json!({
                "abi": abi,
                "template_bytes": size(&l.root.join("target").join(triple).join("release/libara_runtime.so")),
                "script_bytes": size(&l.out.join("script").join(abi).join("Game.so")),
            })
        })
        .collect();
    Ok(json!({
        "lane": "android-aot",
        "ndk": ndk_rev,
        "stages": l.stages,
        "sizes": {
            "per_abi": per_abi,
            "pack_bytes": size(&pack),
            "apk_bytes": size(&apk),
            "aab_bytes": size(&aab),
            "per_device_apks": device_sizes,
        },
        "artifacts": [apk, aab],
        "verify": verify,
    }))
}

fn aapt2_link(
    bt: &Path,
    jar: &Path,
    root: &Path,
    out: &Path,
    proto: bool,
    log: &Path,
) -> Result<()> {
    let mut cmd = Command::new(bt.join(format!("aapt2{}", std::env::consts::EXE_SUFFIX)));
    cmd.arg("link")
        .arg("-o")
        .arg(out)
        .arg("--manifest")
        .arg(root.join("android/AndroidManifest.xml"))
        .arg("-I")
        .arg(jar)
        .args([
            "--min-sdk-version",
            &env::ANDROID_MIN_API.to_string(),
            "--target-sdk-version",
            &env::ANDROID_TARGET_API.to_string(),
            "--version-code",
            "1",
            "--version-name",
            "0.1.0",
        ]);
    if proto {
        cmd.arg("--proto-format");
    }
    exec(&mut cmd, log).map(drop)
}

/// lib/<abi>/{libara_runtime.so, libgame.so} plus assets/game.arapack.
fn add_payload(l: &Lane, dir: &Path, pack: &Path) -> Result<()> {
    for (abi, triple, _) in ANDROID_ABIS {
        let lib = dir.join("lib").join(abi);
        copy(
            &l.root
                .join("target")
                .join(triple)
                .join("release/libara_runtime.so"),
            &lib.join("libara_runtime.so"),
        )?;
        copy(
            &l.out.join("script").join(abi).join("Game.so"),
            &lib.join("libgame.so"),
        )?;
    }
    copy(pack, &dir.join("assets/game.arapack"))
}

/// Installs and launches on the first attached device, then reads the result from logcat.
/// Leaves adb as found: kills the server only if this call started it.
fn android_verify(l: &mut Lane, sdk: &Path, apk: &Path) -> Result<Value> {
    let Some(adb) = env::adb(Some(sdk)) else {
        l.ev.warning(
            "ARA-EXP-AND-011",
            "verify",
            "adb not found; verify skipped",
            &[],
            json!({}),
        );
        return Ok(Value::Null);
    };
    let server_was_up = std::net::TcpStream::connect("127.0.0.1:5037").is_ok();
    let devices = env::capture(&adb, &["devices"]).unwrap_or_default();
    let serial = devices
        .lines()
        .skip(1)
        .find(|l| l.ends_with("device"))
        .and_then(|l| l.split_whitespace().next())
        .map(String::from);
    let result = match serial {
        None => {
            l.ev.warning(
                "ARA-EXP-AND-009",
                "verify",
                "no device attached; APK/AAB built but not run",
                &[Fix::cmd("ara-s7 doctor --platform android --devices")],
                json!({}),
            );
            Ok(Value::Null)
        }
        Some(serial) => l
            .stage("verify", "ARA-EXP-AND-106", &[Fix::cmd(format!("adb -s {serial} logcat -d -s ara"))], |l| {
                let adb_s = |args: &[&str]| -> Command {
                    let mut c = Command::new(&adb);
                    c.args(["-s", &serial]).args(args);
                    c
                };
                let abi = env::capture(&adb, &["-s", &serial, "shell", "getprop", "ro.product.cpu.abi"]).unwrap_or_default();
                let page = env::capture(&adb, &["-s", &serial, "shell", "getconf", "PAGE_SIZE"]).unwrap_or_default();
                exec(adb_s(&["install", "-r", "-t", "--no-incremental"]).arg(apk), &l.log("verify"))?;
                exec(&mut adb_s(&["logcat", "-c"]), &l.log("verify"))?;
                let start = exec(&mut adb_s(&["shell", "am", "start", "-W", "-n", &format!("{PACKAGE}/android.app.NativeActivity")]), &l.log("verify"))?;
                let total_time = start.lines().find_map(|x| x.trim().strip_prefix("TotalTime:")).and_then(|v| v.trim().parse::<u64>().ok());
                let deadline = Instant::now() + Duration::from_secs(90);
                let log = loop {
                    let log = exec(&mut adb_s(&["logcat", "-d", "-v", "brief", "-s", "ara:*", "AndroidRuntime:E", "DEBUG:*", "linker:*"]), &l.log("verify-logcat"))?;
                    if log.contains("ARA_RESULT") || log.contains("fatal") || Instant::now() > deadline {
                        break log;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                };
                let _ = adb_s(&["shell", "am", "force-stop", PACKAGE]).output();
                let mut report = parse_report(&log).with_context(|| format!("no ARA_RESULT in logcat:\n{}", tail(&log, 30)))?;
                report["device"] = json!({"serial": serial, "abi": abi, "page_size": page, "am_start_total_ms": total_time});
                report["logcat"] = json!(log.lines().filter(|x| x.contains("[cs]") || x.contains("[rt]")).collect::<Vec<_>>());
                Ok(report)
            })
            .map(|report| check_report(l.ev, &report, "aot")),
    };
    if !server_was_up {
        let _ = Command::new(&adb).arg("kill-server").output();
    }
    result
}

// ---------------------------------------------------------------- shared

fn cargo(root: &Path) -> Command {
    let mut c = Command::new("cargo");
    c.current_dir(root).env("CARGO_BUILD_JOBS", "4");
    c
}

/// `dotnet publish game/Game.csproj` for `rid`/`lane` into `out`. No build servers left running.
fn dotnet_publish(root: &Path, rid: &str, lane: &str, out: &Path) -> Command {
    let mut c = Command::new("dotnet");
    c.current_dir(root)
        .args([
            "publish",
            "game/Game.csproj",
            "-c",
            "Release",
            "-r",
            rid,
            &format!("-p:AraLane={lane}"),
            "--nologo",
            "--disable-build-servers",
            "-o",
        ])
        .arg(out);
    c
}

fn build_mod(l: &Lane) -> Result<()> {
    exec(
        Command::new("dotnet")
            .current_dir(&l.root)
            .args([
                "build",
                "mod/Mod.csproj",
                "-c",
                "Release",
                "--nologo",
                "--disable-build-servers",
                "-o",
            ])
            .arg(l.out.join("mod")),
        &l.log("mod"),
    )
    .map(drop)
}

fn build_pack(l: &Lane, profile: &str) -> Result<PathBuf> {
    let built = ara_runtime::pack::write::build(&l.root.join("content"), profile)?;
    let path = l.out.join("game.arapack");
    std::fs::write(&path, &built.bytes)?;
    l.ev.artifact(
        "pack",
        &path.to_string_lossy(),
        built.bytes.len() as u64,
        json!({"pack_id": built.id, "file_blake3": blake3::hash(&built.bytes).to_hex().to_string(), "profile": profile}),
    );
    Ok(path)
}

fn summary(l: &Lane, lane: &str, aot_dir: &str, verify: Value) -> Value {
    let size = |p: &Path| dir_size(p).unwrap_or(0);
    let template = ["ara-player.exe", "ara-player"]
        .iter()
        .map(|n| size(&l.dist.join(n)))
        .sum::<u64>();
    let script = if lane != "r2r" {
        size(&l.dist.join(aot_dir))
    } else {
        size(&l.dist.join("dotnet"))
    };
    json!({
        "lane": l.name,
        "stages": l.stages,
        "sizes": {
            "template_bytes": template,
            "script_bytes": script,
            "pack_bytes": size(&l.dist.join("game.arapack")),
            "mods_bytes": size(&l.dist.join("mods")),
            "total_bytes": size(&l.dist),
        },
        "dist": l.dist,
        "verify": verify,
    })
}

/// Runs an exported desktop build with the IL mod and returns its `ARA_RESULT`.
fn run_local(exe: &Path, log: &Path) -> Result<Value> {
    let dir = exe.parent().context("exe has no dir")?;
    let out = exec(
        Command::new(exe)
            .current_dir(dir)
            .args(["--mod", "mods/Mod.dll"]),
        log,
    )?;
    parse_report(&out)
}

fn parse_report(output: &str) -> Result<Value> {
    let line = output
        .lines()
        .find_map(|l| l.split_once("ARA_RESULT ").map(|(_, j)| j))
        .context("no ARA_RESULT line")?;
    Ok(serde_json::from_str(line.trim())?)
}

/// Pass = checksums match, the fault was contained, and mods behave per lane (R2R loads, AOT rejects).
fn check_report(ev: &Events, report: &Value, lane: &str) -> Value {
    let expected_mod = if lane != "r2r" { 2 } else { 0 };
    let pass = report["verified"] == json!(true)
        && report["fault_contained"] == json!(true)
        && (report["mod_status"].is_null() || report["mod_status"] == json!(expected_mod));
    if !pass {
        ev.error(
            "ARA-EXP-VER-001",
            "verify",
            "exported build ran but failed its checks",
            &[],
            report.clone(),
        );
    }
    json!({"pass": pass, "report": report})
}

/// Runs a command, writes its output to `log`, and fails with the output tail on non-zero exit.
fn exec(cmd: &mut Command, log: &Path) -> Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("spawning {:?}", cmd.get_program()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    writeln!(
        f,
        "$ {:?} {:?}\n{text}",
        cmd.get_program(),
        cmd.get_args().collect::<Vec<_>>()
    )?;
    if !out.status.success() {
        bail!(
            "{:?} exited with {}; log {}\n{}",
            cmd.get_program(),
            out.status,
            log.display(),
            tail(&text, 25)
        );
    }
    Ok(text)
}

fn wsl(script: &str, log: &Path) -> Result<String> {
    exec(
        Command::new("wsl.exe").args(["-d", &env::wsl_distro(), "-e", "bash", "-lc", script]),
        log,
    )
}

fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn path_with(dir: Option<&Path>) -> std::ffi::OsString {
    let mut paths: Vec<PathBuf> = dir.map(Path::to_path_buf).into_iter().collect();
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap_or_default()
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e4).round() / 10.0
}

fn remove(p: &Path) -> Result<()> {
    if p.exists() {
        std::fs::remove_dir_all(p).with_context(|| format!("removing {}", p.display()))?;
    }
    Ok(())
}

fn reset(p: &Path) -> Result<()> {
    remove(p)?;
    std::fs::create_dir_all(p)?;
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to)
        .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path, keep: &dyn Fn(&Path) -> bool) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let p = entry?.path();
        let dest = to.join(p.file_name().context("no file name")?);
        if p.is_dir() {
            copy_dir(&p, &dest, keep)?;
        } else if keep(&p) {
            std::fs::copy(&p, &dest)?;
        }
    }
    Ok(())
}

/// Recursive size in bytes (a file's own size if `p` is a file).
pub fn dir_size(p: &Path) -> Result<u64> {
    let meta = std::fs::metadata(p)?;
    if meta.is_file() {
        return Ok(meta.len());
    }
    let mut total = 0;
    for entry in std::fs::read_dir(p)? {
        total += dir_size(&entry?.path())?;
    }
    Ok(total)
}
