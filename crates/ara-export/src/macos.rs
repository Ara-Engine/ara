//! The host-native macOS lane. On an Apple silicon Mac the exporter lays out a runnable
//! `<Game>.app`: the player in `Contents/MacOS`, every data file the player reads in
//! `Contents/Resources` (its install root, `ara_gpu::cooked::install_root`), the game
//! icon and `Info.plist`. It signs the bundle ad-hoc and verifies it by launching the
//! player headless. Other hosts and iOS keep the Mac-agent seam ([`crate::apple`]);
//! Developer ID signing and notarization are not configured yet (`ARA-EXP-APL-007`).
use crate::{ExportPreset, tools, verify::VerifyReport};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// The earliest macOS with Metal 4, also the shader converter's minimum target.
const MINIMUM_SYSTEM: &str = "26.0";
/// The icon a macOS runtime template carries and `Info.plist` names.
pub(crate) const ICON: &str = "AppIcon.icns";
/// The player executable inside the bundle.
pub(crate) const EXECUTABLE: &str = "ara-player";
/// `icns` slots and the pixel size each PNG holds: 1x and 2x from 16 to 512 points.
const ICON_SLOTS: [(&[u8; 4], u32); 11] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
];

/// The app's name: the project's display name, without characters a bundle folder
/// cannot hold; `Game` when the project has none.
pub(crate) fn app_name(project: &Path) -> String {
    let name = std::fs::read(project.join("ara.project.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("name")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | ':' | '\\'))
        .collect();
    let name = name.trim().trim_start_matches('.');
    if name.is_empty() {
        "Game".into()
    } else {
        name.into()
    }
}

/// Lays out `staged`, the desktop build folder (player, launch file, pack, resources,
/// gameplay image, icon), as `<name>.app` in `build`.
pub(crate) fn bundle(
    staged: &Path,
    build: &Path,
    name: &str,
    preset: &ExportPreset,
) -> Result<PathBuf> {
    ensure!(
        staged.join(ICON).is_file(),
        "ARA-EXP-APL-008: the macOS runtime template has no {ICON}; rebuild it with `ara-export runtime-template` from a macOS player and an engine checkout"
    );
    let app = build.join(format!("{name}.app"));
    let contents = app.join("Contents");
    for entry in std::fs::read_dir(staged)? {
        let entry = entry?;
        let target = if entry.file_name() == EXECUTABLE {
            contents.join("MacOS").join(EXECUTABLE)
        } else {
            contents.join("Resources").join(entry.file_name())
        };
        if entry.file_type()?.is_dir() {
            crate::files::copy_all(&entry.path(), &target)?;
        } else {
            tools::copy(&entry.path(), &target)?;
        }
    }
    let player = contents.join("MacOS").join(EXECUTABLE);
    ensure!(player.is_file(), "the build has no {EXECUTABLE}");
    executable(&player)?;
    std::fs::write(
        contents.join("Info.plist"),
        info_plist(name, &preset.package, preset.version_code),
    )?;
    Ok(app)
}

/// Marks the player executable: runtime template ZIPs carry no Unix modes.
fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The bundle's `Info.plist`: identity from the preset's application ID and version
/// code, Metal 4's minimum macOS, and the template's icon.
pub(crate) fn info_plist(name: &str, identifier: &str, version: u32) -> String {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let (name, identifier) = (escape(name), escape(identifier));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>{name}</string>
	<key>CFBundleExecutable</key>
	<string>{EXECUTABLE}</string>
	<key>CFBundleIconFile</key>
	<string>{ICON}</string>
	<key>CFBundleIdentifier</key>
	<string>{identifier}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.games</string>
	<key>LSMinimumSystemVersion</key>
	<string>{MINIMUM_SYSTEM}</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
"#
    )
}

/// An `.icns` with a PNG in every slot macOS reads, from `<stem>-<pixels>.png` files
/// in `directory` (the engine's `assets/brand/icons/game`).
pub(crate) fn icns(directory: &Path, stem: &str) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for (kind, pixels) in ICON_SLOTS {
        let path = directory.join(format!("{stem}-{pixels}.png"));
        let png = std::fs::read(&path)
            .with_context(|| format!("ARA-EXP-APL-008: missing game icon {}", path.display()))?;
        // The IHDR chunk follows the signature; width and height are big-endian.
        ensure!(
            png.len() > 24
                && png.starts_with(b"\x89PNG\r\n\x1a\n")
                && png[16..20] == pixels.to_be_bytes()
                && png[20..24] == pixels.to_be_bytes(),
            "ARA-EXP-APL-008: {} is not a {pixels}x{pixels} PNG",
            path.display()
        );
        body.extend_from_slice(kind);
        body.extend_from_slice(&u32::try_from(png.len() + 8)?.to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut icns = b"icns".to_vec();
    icns.extend_from_slice(&u32::try_from(body.len() + 8)?.to_be_bytes());
    icns.extend(body);
    Ok(icns)
}

/// Signs the bundle ad-hoc (`codesign --sign -`): its gameplay images first, then the
/// app, and checks the seal. The app runs on this Mac; other Macs' Gatekeeper needs
/// Developer ID signing and notarization.
pub(crate) fn sign(app: &Path, logs: &Path) -> Result<()> {
    let game = app.join("Contents/Resources/game");
    let mut code = Vec::new();
    if game.is_dir() {
        for entry in std::fs::read_dir(&game)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "dylib") {
                code.push(path);
            }
        }
    }
    code.sort();
    code.push(app.to_path_buf());
    for (index, path) in code.iter().enumerate() {
        codesign(
            Command::new("codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(path),
            &logs.join(format!("codesign-{index}.log")),
        )?;
    }
    seal(app, &logs.join("codesign-verify.log"))
}

/// Checks the bundle's signature covers every file unchanged.
fn seal(app: &Path, log: &Path) -> Result<()> {
    codesign(
        Command::new("codesign")
            .args(["--verify", "--strict", "--deep"])
            .arg(app),
        log,
    )
}

fn codesign(command: &mut Command, log: &Path) -> Result<()> {
    tools::run(command, log, Duration::from_secs(120))
        .map(drop)
        .with_context(|| {
            format!(
                "ARA-EXP-APL-006: codesign failed; see {} (install the Xcode command-line tools)",
                log.display()
            )
        })
}

/// Launches the bundle's player headless with its capture, crash folder, profile and
/// working directory outside the bundle, so the signed contents never change, then
/// re-checks the seal. Evidence lands in the export root (`output`).
pub(crate) fn verify(app: &Path, work: &Path, output: &Path) -> Result<VerifyReport> {
    let run = work.join("verify");
    std::fs::create_dir_all(&run)?;
    let capture = output.join("verify.png");
    let text = tools::run(
        Command::new(app.join("Contents/MacOS").join(EXECUTABLE))
            .arg("--headless")
            .arg("--capture")
            .arg(&capture)
            .current_dir(&run)
            .env("ARA_CRASH_DIR", run.join("crashes"))
            .env("ARA_CONFIG_DIR", run.join("profile")),
        &output.join("verify.log"),
        Duration::from_secs(120),
    )?;
    let result = crate::verify::runtime_result(&text)?;
    let launch: Value = serde_json::from_slice(&std::fs::read(
        app.join("Contents/Resources/ara-launch.json"),
    )?)?;
    if let Some(contract) = launch.get("contract") {
        ensure!(
            result.get("template_contract") == Some(contract),
            "ARA-EXP-TPL-005: player result does not match the selected template contract"
        );
    }
    ensure!(
        capture.is_file(),
        "ARA-EXP-VER-002: runtime emitted no rendered capture"
    );
    seal(app, &work.join("logs/codesign-after-verify.log"))?;
    Ok(VerifyReport {
        method: "macos_app".into(),
        passed: true,
        runtime_executed: true,
        logs: "verify.log".into(),
        screenshot: Some("verify.png".into()),
        perf: json!({"gpuMs":result.get("gpu_ms"),"quality":result.get("quality"),"elapsedMs":result.get("elapsed_ms")}),
        evidence: result,
        unavailable: Vec::new(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn png(pixels: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend_from_slice(&pixels.to_be_bytes());
        bytes.extend_from_slice(&pixels.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    #[test]
    fn the_icon_holds_every_slot_and_rejects_a_wrong_size() {
        let dir = tempfile::tempdir().unwrap();
        for pixels in [16, 32, 64, 128, 256, 512, 1024] {
            std::fs::write(dir.path().join(format!("game-{pixels}.png")), png(pixels)).unwrap();
        }
        let bytes = icns(dir.path(), "game").unwrap();
        assert_eq!(&bytes[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len()
        );
        let mut at = 8;
        let mut slots = Vec::new();
        while at < bytes.len() {
            let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let width = u32::from_be_bytes(bytes[at + 24..at + 28].try_into().unwrap());
            slots.push((bytes[at..at + 4].to_vec(), width));
            at += len;
        }
        let expected: Vec<_> = ICON_SLOTS.iter().map(|(k, p)| (k.to_vec(), *p)).collect();
        assert_eq!(slots, expected);
        std::fs::write(dir.path().join("game-64.png"), png(48)).unwrap();
        assert!(icns(dir.path(), "game").is_err());
    }

    #[test]
    fn the_plist_names_the_player_icon_and_metal_4_minimum() {
        let plist = info_plist("Salvage & Co", "dev.ara.outpost", 7);
        for line in [
            "<string>Salvage &amp; Co</string>",
            "<string>ara-player</string>",
            "<string>AppIcon.icns</string>",
            "<string>dev.ara.outpost</string>",
            "<string>26.0</string>",
            "<string>7</string>",
        ] {
            assert!(plist.contains(line), "{line} missing from\n{plist}");
        }
    }
}
