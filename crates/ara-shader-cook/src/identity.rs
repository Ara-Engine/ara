//! Cache the expensive SDK digest without changing the canonical cook key.
use super::{Toolchain, hash, run};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(not(target_os = "macos"))]
const RECIPE:&[u8]=b"ara-cook-3|sm_6_6+spirv_1_6|sm_6_6|-O2|column-major|VULKAN|invert-y-vertex-mesh|vulkan1.3-validation";
#[cfg(target_os = "macos")]
const RECIPE: &[u8] = b"ara-cook-3|sm_6_6+spirv_1_6|sm_6_6|-O2|column-major|VULKAN|invert-y-vertex-mesh|vulkan1.3-validation|macos-hlsl-dxc-cli|-O2|-Zpc|-enable-16bit-types|-HV2021";
#[derive(PartialEq, Serialize, Deserialize)]
struct File {
    path: PathBuf,
    bytes: u64,
    modified_ns: u128,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Identity {
    files: Vec<File>,
    abi: String,
    pub version: String,
    pub key: String,
}
fn files(tools: &Toolchain) -> Result<Vec<File>> {
    let executable = tools
        .slangc
        .canonicalize()
        .context("locating Slang compiler for its content fingerprint")?;
    let mut paths = vec![executable.clone()];
    if let Some(bin) = executable.parent() {
        for name in [
            "slang.dll",
            "slang-compiler.dll",
            "slang-glslang.dll",
            "slang-glsl-module.dll",
            "slang-rt.dll",
            "dxcompiler.dll",
        ] {
            let path = bin.join(name);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(bin) = executable.parent() {
        let dxc = bin.join("dxc");
        if dxc.is_file() {
            paths.push(dxc.canonicalize()?);
        }
        if let Some(root) = bin.parent() {
            let lib = root.join("lib");
            if lib.is_dir() {
                let mut libraries = fs::read_dir(lib)?
                    .map(|e| e.map(|e| e.path()))
                    .collect::<std::io::Result<Vec<_>>>()?;
                libraries.retain(|p| {
                    p.file_name().is_some_and(|name| {
                        let name = name.to_string_lossy();
                        (name.starts_with("libslang") || name.starts_with("libdxcompiler"))
                            && name.ends_with(".dylib")
                    })
                });
                libraries.sort();
                for library in libraries {
                    let library = library.canonicalize()?;
                    if !paths.contains(&library) {
                        paths.push(library);
                    }
                }
            }
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let meta = fs::metadata(&path)?;
            Ok(File {
                path,
                bytes: meta.len(),
                modified_ns: meta
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
            })
        })
        .collect()
}
pub(super) fn load(tools: &Toolchain, output: &Path) -> Result<Identity> {
    fs::create_dir_all(output)?;
    let files = files(tools)?;
    let abi = hash([RECIPE, ara_gpu::cooked::binding_layout_hash().as_bytes()].concat());
    let directory = std::env::var_os("ARA_SHADER_TOOL_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| output.to_owned());
    fs::create_dir_all(&directory)?;
    let path = directory.join(".ara-compiler-identity.json");
    if let Ok(bytes) = fs::read(&path)
        && let Ok(cached) = serde_json::from_slice::<Identity>(&bytes)
        && cached.files == files
        && cached.abi == abi
        && cached.key.len() == 64
        && cached.key.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Ok(cached);
    }
    let version = String::from_utf8(run(Command::new(&tools.slangc).arg("-version"))?)?;
    let mut digest = Sha256::new();
    digest.update(RECIPE);
    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            digest.update(
                file.path
                    .file_name()
                    .context("compiler DLL name")?
                    .to_string_lossy()
                    .as_bytes(),
            );
        }
        digest.update(fs::read(&file.path)?);
    }
    digest.update(version.as_bytes());
    digest.update(ara_gpu::cooked::binding_layout_hash());
    let identity = Identity {
        files,
        abi,
        version,
        key: hash(digest.finalize()),
    };
    ara_gpu::cooked::write_json(&path, &identity)?;
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_identity_detects_tool_changes_without_running_a_compiler() -> Result<()> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-shader-cook/tests");
        fs::create_dir_all(&base)?;
        let temp = tempfile::tempdir_in(base)?;
        let exe = temp.path().join("slangc.exe");
        fs::write(&exe, b"compiler")?;
        let tools = Toolchain {
            slangc: exe.clone(),
            spirv_val: temp.path().join("validator"),
        };
        let before = files(&tools)?;
        let identity = Identity {
            files: before,
            abi: hash([RECIPE, ara_gpu::cooked::binding_layout_hash().as_bytes()].concat()),
            version: "cached version".into(),
            key: "1".repeat(64),
        };
        ara_gpu::cooked::write_json(&temp.path().join(".ara-compiler-identity.json"), &identity)?;
        assert_eq!(load(&tools, temp.path())?.version, "cached version");
        fs::write(exe, b"different compiler")?;
        assert!(files(&tools)? != identity.files);
        assert!(load(&tools, temp.path()).is_err());
        Ok(())
    }
}
