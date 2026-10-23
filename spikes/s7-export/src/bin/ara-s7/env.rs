//! Toolchain discovery shared by `doctor` and `export`. Nothing here installs anything.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The spike directory (where `game/`, `content/` and `android/` live).
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// .NET SDK the lanes are pinned to (major version).
pub const DOTNET_MAJOR: u32 = 10;
/// Rust toolchain pin (D17).
pub const RUST_PIN: &str = "1.99.1";
/// Android API the APK targets (Play requirement since 2026-08-31).
pub const ANDROID_TARGET_API: u32 = 36;
/// Android floor (R08: Android 10).
pub const ANDROID_MIN_API: u32 = 29;
/// Oldest NDK major the NativeAOT bionic lane is known to need (r27+).
pub const NDK_MIN_MAJOR: u32 = 27;
/// Steam Runtime 3 "sniper" glibc baseline.
pub const GLIBC_BASELINE: (u32, u32) = (2, 31);

/// Finds `name` on PATH (adds `.exe`/`.cmd`/`.bat` on Windows).
pub fn which(name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| exts.iter().map(move |e| dir.join(format!("{name}{e}"))))
        .find(|p| p.is_file())
}

/// Runs a command and returns trimmed stdout+stderr if it exited 0.
pub fn capture(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text.trim().to_string())
}

/// `wsl.exe` prints UTF-16 for its own commands; this decodes either form.
pub fn wsl_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).take(8).all(|b| *b == 0) {
        let wide: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&wide)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Distro used for the Linux lane (`ARA_WSL_DISTRO`, default `Ubuntu-24.04`).
pub fn wsl_distro() -> String {
    std::env::var("ARA_WSL_DISTRO").unwrap_or_else(|_| "Ubuntu-24.04".into())
}

/// Runs `bash -lc script` in the WSL distro as the default user; returns (ok, combined output).
pub fn wsl_bash(script: &str) -> Option<(bool, String)> {
    let out = Command::new("wsl.exe")
        .args(["-d", &wsl_distro(), "-e", "bash", "-lc", script])
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text.trim().to_string()))
}

/// Translates a Windows path to its `/mnt/<drive>/…` form inside WSL.
pub fn wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    match s.split_once(":/") {
        Some((drive, rest)) => format!("/mnt/{}/{rest}", drive.to_lowercase()),
        None => s,
    }
}

/// `vswhere.exe`: NativeAOT's `findvcvarsall.bat` needs it *on PATH* to find link.exe.
pub fn vswhere() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramFiles(x86)")?;
    let p = PathBuf::from(base).join("Microsoft Visual Studio/Installer/vswhere.exe");
    p.is_file().then_some(p)
}

/// Android SDK: `ANDROID_HOME`, `ANDROID_SDK_ROOT`, then `%LOCALAPPDATA%/Android/Sdk`.
pub fn android_sdk() -> Option<PathBuf> {
    ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
        .chain(std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Android/Sdk")))
        .find(|p| p.join("platform-tools").is_dir() || p.join("build-tools").is_dir())
}

/// Highest-versioned subdirectory of `dir` by numeric components.
fn newest(dir: &Path) -> Option<PathBuf> {
    let key = |p: &PathBuf| -> Vec<u32> {
        p.file_name()
            .map(|n| {
                n.to_string_lossy()
                    .split(['.', '-'])
                    .filter_map(|s| s.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .max_by_key(key)
}

/// NDK root and its `Pkg.Revision`: `ANDROID_NDK_HOME`, else the newest `<sdk>/ndk/*`.
pub fn ndk(sdk: Option<&Path>) -> Option<(PathBuf, String)> {
    let dir = std::env::var_os("ANDROID_NDK_HOME")
        .map(PathBuf::from)
        .or_else(|| newest(&sdk?.join("ndk")))?;
    let props = std::fs::read_to_string(dir.join("source.properties")).ok()?;
    let rev = props
        .lines()
        .find_map(|l| l.strip_prefix("Pkg.Revision"))?
        .trim_start_matches([' ', '='])
        .trim()
        .to_string();
    Some((dir, rev))
}

/// The NDK's prebuilt LLVM `bin` for this host.
pub fn ndk_bin(ndk: &Path) -> PathBuf {
    let host = if cfg!(windows) {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    };
    ndk.join("toolchains/llvm/prebuilt").join(host).join("bin")
}

/// Newest `<sdk>/build-tools/*` (aapt2, zipalign, apksigner).
pub fn build_tools(sdk: &Path) -> Option<PathBuf> {
    newest(&sdk.join("build-tools"))
}

/// `<sdk>/platforms/android-<api>/android.jar` for the target API.
pub fn platform_jar(sdk: &Path) -> Option<PathBuf> {
    let p = sdk.join(format!(
        "platforms/android-{ANDROID_TARGET_API}/android.jar"
    ));
    p.is_file().then_some(p)
}

/// A JDK tool (`java`, `keytool`, `jar`, `jarsigner`) from `JAVA_HOME`, else PATH.
pub fn jdk_tool(name: &str) -> Option<PathBuf> {
    std::env::var_os("JAVA_HOME")
        .map(|h| {
            PathBuf::from(h).join("bin").join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            })
        })
        .filter(|p| p.is_file())
        .or_else(|| which(name))
}

/// bundletool jar: `ARA_BUNDLETOOL`, else `tools/bundletool.jar` in the spike.
pub fn bundletool() -> Option<PathBuf> {
    std::env::var_os("ARA_BUNDLETOOL")
        .map(PathBuf::from)
        .or_else(|| Some(root().join("tools/bundletool.jar")))
        .filter(|p| p.is_file())
}

/// `adb` from the SDK, else PATH.
pub fn adb(sdk: Option<&Path>) -> Option<PathBuf> {
    sdk.map(|s| {
        s.join(if cfg!(windows) {
            "platform-tools/adb.exe"
        } else {
            "platform-tools/adb"
        })
    })
    .filter(|p| p.is_file())
    .or_else(|| which("adb"))
}

/// Parses the first `major.minor[.patch]` in `text`.
pub fn version_in(text: &str) -> Option<Vec<u32>> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|s| s.contains('.') && s.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|s| s.split('.').filter_map(|p| p.parse().ok()).collect())
}
