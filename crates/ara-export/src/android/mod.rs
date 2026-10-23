//! Gradle-free Android APK/AAB packaging and signing.
mod archive;
mod audit;
mod manifest;
use crate::{
    ExportPreset,
    tools::{self, Toolchain},
};
use anyhow::{Context, Result, ensure};
use archive::write_zip;
pub use archive::{deterministic_zip, zip_entries};
pub use audit::{validate_elf, validate_package};
pub use manifest::merge_manifest;
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
/// The two required Android shipping ABIs and corresponding Rust/.NET targets.
pub const ABIS: [(&str, &str, &str); 2] = [
    ("arm64-v8a", "aarch64-linux-android", "linux-bionic-arm64"),
    ("x86_64", "x86_64-linux-android", "linux-bionic-x64"),
];

/// APK and AAB paths returned only after signing and structure validation.
pub struct Packages {
    /// Signed universal APK.
    pub apk: PathBuf,
    /// Signed Play App Bundle.
    pub aab: PathBuf,
}

/// Builds and signs both package formats without Gradle, using fixed ZIP inputs.
pub fn package(
    project: &Path,
    preset: &ExportPreset,
    t: &Toolchain,
    payload: &Path,
    work: &Path,
    dist: &Path,
) -> Result<Packages> {
    let bt = t
        .build_tools
        .as_ref()
        .context("ARA-EXP-AND-003: build-tools missing")?;
    let java = t
        .java_tool("java")
        .context("ARA-EXP-AND-005: java missing")?;
    let bundle = t
        .bundletool
        .as_ref()
        .context("ARA-EXP-AND-006: bundletool missing; both APK and AAB are required")?;
    let platform = t
        .sdk
        .as_ref()
        .context("ARA-EXP-AND-001: SDK missing")?
        .join(format!(
            "platforms/android-{}/android.jar",
            preset.target_api
        ));
    let plugins = preset
        .permission_manifests
        .iter()
        .map(|p| Ok(std::fs::read_to_string(tools::project_path(project, p)?)?))
        .collect::<Result<Vec<_>>>()?;
    let manifest = work.join("AndroidManifest.xml");
    std::fs::write(&manifest, merge_manifest(preset, &plugins)?)?;
    let execute = |c: &mut Command, name: &str| {
        tools::run(
            c,
            &work.join(format!("{name}.log")),
            Duration::from_secs(180),
        )
    };
    let mut base = None;
    for proto in [false, true] {
        let res = work.join(if proto { "proto.apk" } else { "res.apk" });
        let mut cmd = Command::new(bt.join(tools::executable("aapt2")));
        cmd.args(["link", "-o"])
            .arg(&res)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-I")
            .arg(&platform);
        if proto {
            cmd.arg("--proto-format");
        }
        execute(&mut cmd, if proto { "aapt2-proto" } else { "aapt2" })?;
        let mut entries = zip_entries(&res)?;
        for (abi, _, _) in ABIS {
            let dir = payload.join("lib").join(abi);
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                ensure!(entry.file_type()?.is_file(), "invalid Android payload");
                entries.insert(
                    format!("lib/{abi}/{}", entry.file_name().to_string_lossy()),
                    std::fs::read(entry.path())?,
                );
            }
        }
        entries.insert(
            "assets/game.arapack".into(),
            std::fs::read(payload.join("game.arapack"))?,
        );
        entries.insert(
            "assets/ara-launch.json".into(),
            std::fs::read(payload.join("ara-launch.json"))?,
        );
        if proto {
            let xml = entries
                .remove("AndroidManifest.xml")
                .context("missing protobuf manifest")?;
            entries.insert("manifest/AndroidManifest.xml".into(), xml);
            let module = work.join("base.zip");
            deterministic_zip(&entries, &module)?;
            base = Some(module);
        } else {
            deterministic_zip(&entries, &work.join("unaligned.apk"))?;
        }
    }
    let aligned = work.join("aligned.apk");
    execute(
        Command::new(bt.join(tools::executable("zipalign")))
            .args(["-P", "16", "-f", "4"])
            .arg(work.join("unaligned.apk"))
            .arg(&aligned),
        "zipalign",
    )?;
    let (keystore, alias, store, key) = if let Some(s) = &preset.signing {
        ensure!(
            std::env::var_os(&s.store_password_env).is_some()
                && std::env::var_os(&s.key_password_env).is_some(),
            "ARA-EXP-AND-007: signing password environment variables are unset"
        );
        (
            tools::project_path(project, &s.keystore)?,
            s.alias.clone(),
            format!("env:{}", s.store_password_env),
            format!("env:{}", s.key_password_env),
        )
    } else {
        let ks = work
            .parent()
            .context("work has no parent")?
            .join("debug.keystore");
        if !ks.is_file() {
            execute(
                Command::new(t.java_tool("keytool").context("keytool missing")?)
                    .args(["-genkeypair", "-keystore"])
                    .arg(&ks)
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
                        "CN=Android Debug,O=Ara,C=US",
                    ]),
                "keytool",
            )?;
        }
        (
            ks,
            "androiddebugkey".into(),
            "pass:android".into(),
            "pass:android".into(),
        )
    };
    let apk = dist.join("game.apk");
    let aab = dist.join("game.aab");
    let signer = bt.join("lib/apksigner.jar");
    execute(
        Command::new(&java)
            .arg("-jar")
            .arg(&signer)
            .args(["sign", "--ks"])
            .arg(&keystore)
            .args([
                "--ks-key-alias",
                &alias,
                "--ks-pass",
                &store,
                "--key-pass",
                &key,
                "--out",
            ])
            .arg(&apk)
            .arg(&aligned),
        "apksigner-sign",
    )?;
    execute(
        Command::new(&java)
            .arg("-jar")
            .arg(&signer)
            .args(["verify", "--verbose"])
            .arg(&apk),
        "apksigner-verify",
    )?;
    execute(
        Command::new(bt.join(tools::executable("zipalign")))
            .args(["-c", "-P", "16", "4"])
            .arg(&apk),
        "zipalign-verify",
    )?;
    let config = work.join("BundleConfig.json");
    tools::write_json(
        &config,
        &serde_json::json!({"compression":{"uncompressedGlob":["assets/**","lib/**"]}}),
    )?;
    let unsigned = work.join("unsigned.aab");
    execute(
        Command::new(&java)
            .arg("-jar")
            .arg(bundle)
            .arg("build-bundle")
            .arg(format!(
                "--modules={}",
                base.context("missing base module")?.display()
            ))
            .arg(format!("--output={}", unsigned.display()))
            .arg(format!("--config={}", config.display()))
            .arg("--overwrite"),
        "bundletool-build",
    )?;
    write_zip(
        &zip_entries(&unsigned)?,
        &aab,
        zip::CompressionMethod::Deflated,
    )?;
    let mut js = Command::new(t.java_tool("jarsigner").context("jarsigner missing")?);
    js.arg("-keystore").arg(&keystore);
    for (flag, secret) in [("-storepass", &store), ("-keypass", &key)] {
        if let Some(env) = secret.strip_prefix("env:") {
            js.arg(format!("{flag}:env")).arg(env);
        } else {
            js.arg(flag).arg("android");
        }
    }
    js.args(["-sigalg", "SHA256withRSA", "-digestalg", "SHA-256"])
        .arg(&aab)
        .arg(&alias);
    execute(&mut js, "jarsigner-sign")?;
    execute(
        Command::new(t.java_tool("jarsigner").context("jarsigner missing")?)
            .args(["-verify"])
            .arg(&aab),
        "jarsigner-verify",
    )?;
    execute(
        Command::new(&java)
            .arg("-jar")
            .arg(bundle)
            .arg("validate")
            .arg(format!("--bundle={}", aab.display())),
        "bundletool-validate",
    )?;
    validate_package(&apk, false)?;
    validate_package(&aab, true)?;
    Ok(Packages { apk, aab })
}
