//! Private symbol archive generation; shipped payloads never contain debug files.
use crate::{
    ExportPreset, Platform,
    preset::ScriptLane,
    tools::{self, Toolchain},
};
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Finds an LLVM stripping tool without making it a Cargo build requirement.
pub fn objcopy(t: &Toolchain) -> Option<PathBuf> {
    std::env::var_os("ARA_OBJCOPY")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| tools::which("llvm-objcopy"))
        .or_else(|| {
            t.ndk_bin()
                .map(|p| p.join(tools::executable("llvm-objcopy")))
                .filter(|p| p.is_file())
        })
}
fn strip(image: &Path, toolchain: &Toolchain, work: &Path) -> Result<()> {
    let tool = objcopy(toolchain).context("ARA-EXP-CRASH-007: set ARA_OBJCOPY to llvm-objcopy to strip archived DWARF from shipping images")?;
    tools::run(
        Command::new(tool).arg("--strip-debug").arg(image),
        &work.join("logs/strip.log"),
        Duration::from_secs(60),
    )?;
    Ok(())
}

pub(crate) struct Inputs<'a> {
    pub preset: &'a ExportPreset,
    pub templates: &'a BTreeMap<String, PathBuf>,
    pub scripts: &'a BTreeMap<String, PathBuf>,
    pub plugins: &'a [(String, PathBuf)],
    pub project: &'a Path,
    pub cache: &'a Path,
    pub work: &'a Path,
    pub tools: &'a Toolchain,
}

pub(crate) fn archive(inputs: Inputs<'_>, build: &Path, output: &Path) -> Result<String> {
    let Inputs {
        preset: p,
        templates,
        scripts,
        plugins,
        project,
        cache,
        work,
        tools: toolchain,
    } = inputs;
    let identities = crate::distribution::identities(build)?;
    let build_id = blake3::hash(
        ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(&identities)?))
            .as_bytes(),
    )
    .to_hex()
    .to_string();
    let root = output.join("symbols").join(&build_id);
    let store = root.join("store");
    std::fs::create_dir_all(&store)?;
    for pin in &p.templates {
        let raw = root.join("raw").join(&pin.arch).join("runtime");
        std::fs::create_dir_all(&raw)?;
        if p.platform == Platform::Windows {
            let symbols = pin.symbols.as_ref().context("ARA-EXP-CRASH-005: Windows template requires a hash-pinned PDB in templates[].symbols")?;
            let pdb = crate::template::resolve_artifact(symbols, project, cache)?;
            ara_crash::symbols::archive_pe(&templates[&pin.arch], &pdb, &raw, &store)?;
        } else if p.platform == Platform::Macos {
            let symbols = pin.symbols.as_ref().context("ARA-EXP-CRASH-005: macOS template requires its player's hash-pinned dSYM DWARF file in templates[].symbols")?;
            let dwarf = crate::template::resolve_artifact(symbols, project, cache)?;
            ara_crash::symbols::archive_macho(&templates[&pin.arch], &dwarf, &store)?;
        } else {
            let name = if p.platform == Platform::Linux {
                "ara-player"
            } else {
                "libara_export_player.so"
            };
            let original = raw.join(name);
            tools::copy(&templates[&pin.arch], &original)?;
            ara_crash::symbols::archive_elf(&original, &store)?;
            let image = if p.platform == Platform::Linux {
                build.join(name)
            } else {
                build.join("lib").join(&pin.arch).join(name)
            };
            strip(&image, toolchain, work)?;
        }
    }
    for (arch, script) in scripts {
        let raw = root.join("raw").join(arch).join("scripts");
        std::fs::create_dir_all(&raw)?;
        if p.lane == ScriptLane::NativeAot {
            let name = format!(
                "{}.{}",
                p.assembly,
                match p.platform {
                    Platform::Windows => "dll",
                    Platform::Macos => "dylib",
                    _ => "so",
                }
            );
            let image = script.join(&name);
            if p.platform == Platform::Windows {
                let pdb = script.join(format!("{}.pdb", p.assembly));
                ara_crash::symbols::archive_pe(&image, &pdb, &raw, &store)?;
            } else if p.platform == Platform::Macos {
                // NativeAOT splits macOS symbols into a dSYM beside the image.
                let dwarf = script.join(format!("{name}.dSYM/Contents/Resources/DWARF/{name}"));
                ara_crash::symbols::archive_macho(&image, &dwarf, &store)?;
            } else {
                let original = raw.join(&name);
                tools::copy(&image, &original)?;
                ara_crash::symbols::archive_elf(&original, &store)?;
                let deployed = if p.platform == Platform::Linux {
                    build.join("game").join(name)
                } else {
                    build.join("lib").join(arch).join("libgame.so")
                };
                strip(&deployed, toolchain, work)?;
            }
        } else {
            // Portable managed PDBs are archived verbatim; the managed report
            // carries its CLR stack. They are not native PDB input to dump_syms.
            for (name, path) in crate::distribution::files(script)? {
                if path.extension().is_some_and(|e| e == "pdb" || e == "dbg") {
                    tools::copy(&path, &raw.join(name))?;
                }
            }
        }
    }
    for (arch, path) in plugins {
        let name = path.file_name().context("native plugin name missing")?;
        let raw = root.join("raw").join(arch).join("plugins").join(name);
        std::fs::create_dir_all(&raw)?;
        if p.platform == Platform::Windows {
            let (pdb, _) = ara_crash::symbols::pe_debug_identity(path)?;
            ara_crash::symbols::archive_pe(path, &path.with_file_name(pdb), &raw, &store)?;
        } else {
            let original = raw.join(name);
            tools::copy(path, &original)?;
            ara_crash::symbols::archive_elf(&original, &store)?;
            strip(
                &if p.platform == Platform::Android {
                    build.join("lib").join(arch).join(name)
                } else {
                    build.join("plugins").join(name)
                },
                toolchain,
                work,
            )?;
        }
    }
    tools::write_json(
        &root.join("index.json"),
        &serde_json::json!({"format":"ara.symbols/1","buildId":build_id,"files":crate::distribution::identities(&root)?}),
    )?;
    crate::distribution::portable_zip(
        &root,
        &output.join("symbols").join(format!("{build_id}.zip")),
    )?;
    tools::write_json(
        &build.join("ara-crash.json"),
        &serde_json::json!({"format":"ara.crash.config/1","buildId":build_id,"settings":p.crash}),
    )?;
    Ok(build_id)
}
