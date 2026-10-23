use anyhow::{Context, Result, bail};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Detected host tools shared by doctor and export, with explicit environment overrides.
#[derive(Debug, Clone)]
pub struct Toolchain {
    /// Android SDK (`ANDROID_HOME`/`ANDROID_SDK_ROOT`).
    pub sdk: Option<PathBuf>,
    /// Android NDK installation.
    pub ndk: Option<PathBuf>,
    /// Android build-tools directory.
    pub build_tools: Option<PathBuf>,
    /// Java installation root.
    pub java_home: Option<PathBuf>,
    /// Pinned Zig binary (`ARA_ZIG`).
    pub zig: Option<PathBuf>,
    /// Production linker wrapper binary (`ARA_ZIGCC` or sibling ara-zigcc).
    pub zigcc: Option<PathBuf>,
    /// bundletool jar (`ARA_BUNDLETOOL`).
    pub bundletool: Option<PathBuf>,
    /// MSVC locator needed by .NET's AOT link targets.
    pub vswhere: Option<PathBuf>,
}
impl Toolchain {
    /// Discovers files only; no tool installation, emulator, WSL or daemon starts.
    pub fn discover() -> Self {
        let sdk = env_path("ANDROID_HOME")
            .or_else(|| env_path("ANDROID_SDK_ROOT"))
            .or_else(|| env_path("LOCALAPPDATA").map(|p| p.join("Android/Sdk")))
            .filter(|p| p.is_dir());
        let ndk = env_path("ANDROID_NDK_HOME").or_else(|| newest(&sdk.as_ref()?.join("ndk")));
        let build_tools = sdk.as_ref().and_then(|s| newest(&s.join("build-tools")));
        let vswhere = env_path("ProgramFiles(x86)")
            .map(|p| p.join("Microsoft Visual Studio/Installer/vswhere.exe"))
            .filter(|p| p.is_file());
        let zigcc = env_path("ARA_ZIGCC")
            .or_else(|| {
                std::env::current_exe()
                    .ok()?
                    .parent()
                    .map(|p| p.join(format!("ara-zigcc{}", std::env::consts::EXE_SUFFIX)))
            })
            .filter(|p| p.is_file());
        Self {
            sdk,
            ndk,
            build_tools,
            java_home: env_path("JAVA_HOME"),
            zig: env_path("ARA_ZIG").or_else(|| which("zig")),
            zigcc,
            bundletool: env_path("ARA_BUNDLETOOL").filter(|p| p.is_file()),
            vswhere,
        }
    }
    /// Native host LLVM tools directory within the NDK.
    pub fn ndk_bin(&self) -> Option<PathBuf> {
        Some(
            self.ndk
                .as_ref()?
                .join("toolchains/llvm/prebuilt")
                .join(if cfg!(windows) {
                    "windows-x86_64"
                } else if cfg!(target_os = "macos") {
                    "darwin-x86_64"
                } else {
                    "linux-x86_64"
                })
                .join("bin"),
        )
    }
    /// JDK executable with PATH fallback.
    pub fn java_tool(&self, name: &str) -> Option<PathBuf> {
        self.java_home
            .as_ref()
            .map(|p| p.join("bin").join(executable(name)))
            .filter(|p| p.is_file())
            .or_else(|| which(name))
    }
    /// Android build-tools executable.
    pub fn android_tool(&self, name: &str) -> Option<PathBuf> {
        self.build_tools
            .as_ref()
            .map(|p| p.join(executable(name)))
            .filter(|p| p.is_file())
    }
    /// SDK's adb, preferred to conflicting PATH installations.
    pub fn adb(&self) -> Option<PathBuf> {
        self.sdk
            .as_ref()
            .map(|s| s.join("platform-tools").join(executable("adb")))
            .filter(|p| p.is_file())
            .or_else(|| which("adb"))
    }
    /// PATH with local tool directories first, without changing the parent environment.
    pub fn path(&self) -> Result<OsString> {
        let mut paths = Vec::new();
        if let Some(p) = &self.vswhere {
            paths.extend(p.parent().map(Path::to_path_buf));
        }
        if let Some(p) = &self.zigcc {
            paths.extend(p.parent().map(Path::to_path_buf));
        }
        paths.extend(self.ndk_bin());
        if let Some(p) = &self.java_home {
            paths.push(p.join("bin"));
        }
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        Ok(std::env::join_paths(paths)?)
    }
}

/// Finds an executable on the current PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|p| {
            [name.to_owned(), executable(name)]
                .into_iter()
                .map(move |n| p.join(n))
        })
        .find(|p| p.is_file())
}
/// Adds the native executable suffix.
pub fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}
fn newest(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .max_by_key(|p| {
            p.file_name()
                .map(|n| version(&n.to_string_lossy()))
                .unwrap_or_default()
        })
}
/// Parses numeric version components, handling quoted Java and tool banners.
pub fn version(text: &str) -> Vec<u32> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.')
        .find(|s| s.contains('.') && s.starts_with(|c: char| c.is_ascii_digit()))
        .map(|s| s.split('.').filter_map(|v| v.parse().ok()).collect())
        .unwrap_or_default()
}

/// Runs an out-of-process compiler/tool below normal priority with a deadline.
/// Logs contain tool output only; arguments and environment may contain secrets.
pub fn run(command: &mut Command, log: &Path, timeout: Duration) -> Result<String> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(log)?;
    command
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file);
    command.env("CARGO_BUILD_JOBS", "2");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00004000 | 0x08000000);
    }
    let mut child = command.spawn().with_context(|| {
        format!(
            "spawning {:?}; log {}",
            command.get_program(),
            log.display()
        )
    })?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill.exe")
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(ToolFailure {
                program: command.get_program().to_string_lossy().into_owned(),
                log: log.into(),
                exit_code: None,
                timed_out: true,
            }
            .into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = std::fs::read_to_string(log).unwrap_or_else(|_| "[non-UTF8 tool output]".into());
    if !status.success() {
        return Err(ToolFailure {
            program: command.get_program().to_string_lossy().into_owned(),
            log: log.into(),
            exit_code: status.code(),
            timed_out: false,
        }
        .into());
    }
    Ok(output)
}

/// Structured external-tool failure carried into export error context.
#[derive(Debug)]
pub struct ToolFailure {
    /// Executable name/path; arguments are deliberately excluded.
    pub program: String,
    /// Output log containing the tool's diagnostics.
    pub log: PathBuf,
    /// Process exit code, or absent for timeout/signal.
    pub exit_code: Option<i32>,
    /// True if the export deadline killed the process tree.
    pub timed_out: bool,
}
impl std::fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} failed (exit {:?}, timeout {}); log {}",
            if self.timed_out {
                "ARA-EXP-TOOL-002"
            } else {
                "ARA-EXP-TOOL-001"
            },
            self.program,
            self.exit_code,
            self.timed_out,
            self.log.display()
        )
    }
}
impl std::error::Error for ToolFailure {}

/// Runs a short read-only probe with a 10-second deadline.
pub fn probe(
    program: impl AsRef<OsStr>,
    args: &[&str],
    cache: &Path,
    name: &str,
) -> Option<String> {
    run(
        Command::new(program).args(args),
        &cache.join(format!("{name}.log")),
        Duration::from_secs(10),
    )
    .ok()
}

/// Canonical JSON for metadata with no registered schema-defined order.
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    std::fs::write(
        path,
        ara_json::to_canonical_string(&ara_json::sort_keys(&ara_json::to_value(value)?)),
    )?;
    Ok(())
}

/// Copies one regular file, creating destination parents.
pub fn copy(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, target)
        .with_context(|| format!("{} -> {}", source.display(), target.display()))?;
    Ok(())
}

/// Resolves a project path and rejects symlink/junction escapes.
pub fn project_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ara_pack::validate_path(relative)?;
    let path = dunce::canonicalize(root.join(relative))?;
    if !path.starts_with(dunce::canonicalize(root)?) {
        bail!("ARA-EXP-PRE-006: project path escapes the project");
    }
    Ok(path)
}
