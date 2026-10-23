use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use ara_diag::Diagnostic;

use crate::{Generated, codes};

/// Offline shader artifact target; Metal conversion consumes the DXIL lane later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderTarget {
    /// Vulkan 1.3 SPIR-V.
    Spirv,
    /// Shader Model 6.6 DXIL for Metal Shader Converter (or future D3D12).
    Dxil,
}

/// Cooked shader bytes, with compiler identity for content/PSO cache keys.
#[derive(Debug)]
pub struct CompiledShader {
    /// Target bytecode format.
    pub target: ShaderTarget,
    /// Validated binary magic followed by the compiler's complete artifact.
    pub bytes: Vec<u8>,
    /// Exact compiler version output; include this and ABI/template hashes in cook keys.
    pub compiler_version: String,
    /// Nonfatal compiler output, preserved for the diagnostics UI.
    pub messages: String,
}

/// Explicit external compiler. No render/editor thread should call this synchronously.
#[derive(Debug)]
pub struct ShaderCompiler {
    /// slangc executable; discovery uses SLANGC, VULKAN_SDK, then PATH.
    pub executable: PathBuf,
}

fn error(e: impl ToString) -> Diagnostic {
    Diagnostic::error(codes::COMPILER, e.to_string())
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
static NEXT: AtomicU64 = AtomicU64::new(0);

impl ShaderCompiler {
    fn command(&self) -> Command {
        let command = Command::new(&self.executable);
        #[cfg(windows)]
        let command = {
            use std::os::windows::process::CommandExt;
            let mut command = command;
            // BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW, including version probes.
            command.creation_flags(0x00004000 | 0x08000000);
            command
        };
        command
    }

    /// Resolves the installed offline compiler; does not launch it.
    pub fn discover() -> Self {
        let executable = std::env::var_os("SLANGC")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("VULKAN_SDK").map(|sdk| {
                    PathBuf::from(sdk)
                        .join(if cfg!(windows) { "Bin" } else { "bin" })
                        .join(if cfg!(windows) {
                            "slangc.exe"
                        } else {
                            "slangc"
                        })
                })
            })
            .unwrap_or_else(|| PathBuf::from("slangc"));
        Self { executable }
    }

    /// Spawns slangc below normal priority on Windows, returning an offline artifact.
    /// The caller owns worker scheduling, cancellation and last-good frame swaps (D20).
    pub fn compile(
        &self,
        generated: &Generated,
        target: ShaderTarget,
    ) -> Result<CompiledShader, Diagnostic> {
        let version = self.command().arg("-version").output().map_err(error)?;
        if !version.status.success() {
            return Err(error("slangc -version failed"));
        }
        let compiler_version = format!(
            "{}{}",
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
        let directory = std::env::temp_dir().join(format!(
            "ara-material-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).map_err(error)?;
        let directory = Temporary(directory);
        let input = directory.0.join("material.slang");
        let output = directory.0.join("material.bin");
        std::fs::write(&input, &generated.source).map_err(error)?;
        let mut command = self.command();
        command
            .arg(&input)
            .args([
                "-entry",
                "fragmentMain",
                "-stage",
                "fragment",
                "-profile",
                "sm_6_6",
                "-matrix-layout-row-major",
            ])
            .args([
                "-target",
                match target {
                    ShaderTarget::Spirv => "spirv",
                    ShaderTarget::Dxil => "dxil",
                },
            ])
            .arg("-o")
            .arg(&output);
        if target == ShaderTarget::Spirv {
            command.args(["-D", "VULKAN=1"]);
        }
        let result = command.output().map_err(error)?;
        let messages = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        if !result.status.success() {
            return Err(error(format!("slangc {target:?} failed: {messages}")));
        }
        let bytes = std::fs::read(&output).map_err(error)?;
        let magic: &[u8] = match target {
            ShaderTarget::Spirv => &[3, 2, 35, 7],
            ShaderTarget::Dxil => b"DXBC",
        };
        if bytes.len() < 20 || !bytes.starts_with(magic) {
            return Err(error("slangc produced an invalid target artifact"));
        }
        Ok(CompiledShader {
            target,
            bytes,
            compiler_version,
            messages,
        })
    }
}
