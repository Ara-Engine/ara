//! ELF and Android package validation, independent of installed SDK tools.
use super::ABIS;
use anyhow::{Context, Result, ensure};
use std::{io::Read, path::Path};
/// Validates ELF64 load-segment alignment for Android's 16 KiB page devices.
pub fn validate_elf(bytes: &[u8], machine: u16, alignment: u64) -> Result<()> {
    ensure!(
        bytes.len() >= 64 && &bytes[..4] == b"\x7fELF" && bytes[4] == 2 && bytes[5] == 1,
        "ARA-EXP-ELF-001: expected little-endian ELF64"
    );
    let u16at = |at| -> Result<u16> {
        Ok(u16::from_le_bytes(
            bytes.get(at..at + 2).context("short ELF")?.try_into()?,
        ))
    };
    let u64at = |at| -> Result<u64> {
        Ok(u64::from_le_bytes(
            bytes.get(at..at + 8).context("short ELF")?.try_into()?,
        ))
    };
    ensure!(u16at(18)? == machine, "ARA-EXP-ELF-002: wrong architecture");
    let start = usize::try_from(u64at(32)?)?;
    let stride = usize::from(u16at(54)?);
    let count = usize::from(u16at(56)?);
    ensure!(
        stride >= 56 && count > 0 && count < 65535,
        "ARA-EXP-ELF-001: invalid program headers"
    );
    let mut loads = 0;
    for i in 0..count {
        let at = start
            .checked_add(i.checked_mul(stride).context("ELF overflow")?)
            .context("ELF overflow")?;
        let p = bytes
            .get(at..at.checked_add(56).context("ELF overflow")?)
            .context("ELF program header out of range")?;
        if u32::from_le_bytes(p[..4].try_into()?) != 1 {
            continue;
        }
        loads += 1;
        let offset = u64::from_le_bytes(p[8..16].try_into()?);
        let address = u64::from_le_bytes(p[16..24].try_into()?);
        let align = u64::from_le_bytes(p[48..56].try_into()?);
        let filesz = u64::from_le_bytes(p[32..40].try_into()?);
        ensure!(
            offset
                .checked_add(filesz)
                .is_some_and(|end| end <= bytes.len() as u64),
            "ARA-EXP-ELF-001: load segment outside file"
        );
        ensure!(
            align >= alignment
                && align.is_power_of_two()
                && offset % alignment == address % alignment,
            "ARA-EXP-AND-024: ELF LOAD segment is not {alignment}-byte aligned"
        );
    }
    ensure!(loads > 0, "ARA-EXP-ELF-001: no load segments");
    Ok(())
}

/// Validates required APK/AAB payloads and APK mmap alignment after signing.
pub fn validate_package(path: &Path, aab: bool) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let prefix = if aab { "base/" } else { "" };
    let manifest = format!(
        "{prefix}{}AndroidManifest.xml",
        if aab { "manifest/" } else { "" }
    );
    ensure!(
        zip.by_name(&manifest).is_ok(),
        "ARA-EXP-AND-025: missing manifest"
    );
    for (abi, _, _) in ABIS {
        for name in ["libara_export_player.so", "libgame.so"] {
            let mut file = zip.by_name(&format!("{prefix}lib/{abi}/{name}"))?;
            if !aab {
                ensure!(
                    file.compression() == zip::CompressionMethod::Stored
                        && file.data_start() % 16384 == 0,
                    "ARA-EXP-AND-026: native library ZIP entry must be stored and 16 KiB aligned"
                );
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            validate_elf(&bytes, if abi == "arm64-v8a" { 183 } else { 62 }, 16384)?;
        }
    }
    let pack = zip.by_name(&format!("{prefix}assets/game.arapack"))?;
    if !aab {
        ensure!(
            pack.compression() == zip::CompressionMethod::Stored && pack.data_start() % 4 == 0,
            "ARA-EXP-AND-027: pack must be stored and aligned for mmap"
        );
    }
    drop(pack);
    if aab {
        ensure!(
            zip.by_name("BundleConfig.pb").is_ok(),
            "ARA-EXP-AND-028: missing bundle config"
        );
    }
    Ok(())
}
