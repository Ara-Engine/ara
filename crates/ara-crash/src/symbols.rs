//! Offline symbol conversion and minidump processing; never linked into a player.
use anyhow::{Context, Result};
use std::path::Path;

/// Reads the PDB basename and GUID/age from a PE image, for build-match validation.
pub fn pe_debug_identity(image: &Path) -> Result<(String, String)> {
    let bytes = std::fs::read(image)?;
    let pe = goblin::pe::PE::parse(&bytes)?;
    let debug = pe.debug_data.and_then(|d| d.codeview_pdb70_debug_info).context("ARA-EXP-CRASH-005: PE image has no PDB identity; build the template with debug information")?;
    let filename = std::str::from_utf8(debug.filename)?
        .trim_end_matches('\0')
        .rsplit(['/', '\\'])
        .next()
        .context("PDB basename missing")?
        .to_string();
    anyhow::ensure!(
        !filename.contains(['/', '\\', ':'])
            && filename != "."
            && filename != ".."
            && !filename.is_empty(),
        "invalid PDB name"
    );
    let guid = debug.signature;
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut id = order
        .iter()
        .map(|i| format!("{:02X}", guid[*i]))
        .collect::<String>();
    id.push_str(&format!("{:X}", debug.age));
    Ok((filename, id))
}

/// Archives a matching Windows PDB. A mismatched GUID/age fails the export.
pub fn archive_pe(image: &Path, pdb: &Path, raw: &Path, store: &Path) -> Result<()> {
    let (name, id) = pe_debug_identity(image)?;
    std::fs::create_dir_all(raw)?;
    let target = raw.join(&name);
    std::fs::copy(pdb, &target)?;
    // On Windows x64, unwind information lives in the PE image, not the PDB.
    // dump_syms finds that image beside the PDB using the same stem and GUID.
    let image_bytes = std::fs::read(image)?;
    let extension = if goblin::pe::PE::parse(&image_bytes)?.is_lib {
        "dll"
    } else {
        "exe"
    };
    std::fs::copy(image, target.with_extension(extension))?;
    archive(&target, store)?;
    let stem = Path::new(&name)
        .file_stem()
        .and_then(|s| s.to_str())
        .context("invalid PDB filename")?;
    let symbol_name = format!("{stem}.sym");
    anyhow::ensure!(
        store.join(&name).join(&id).join(symbol_name).is_file(),
        "ARA-EXP-CRASH-005: PDB does not match image GUID/age"
    );
    Ok(())
}

/// Archives an unstripped ELF, rejecting runtime pins without DWARF.
pub fn archive_elf(image: &Path, store: &Path) -> Result<()> {
    let bytes = std::fs::read(image)?;
    let elf = goblin::elf::Elf::parse(&bytes)?;
    anyhow::ensure!(
        elf.section_headers
            .iter()
            .any(|h| elf.shdr_strtab.get_at(h.sh_name) == Some(".debug_info")),
        "ARA-EXP-CRASH-005: template/image lacks DWARF; prepare with CARGO_PROFILE_RELEASE_DEBUG=line-tables-only and STRIP=none"
    );
    archive(image, store)
}

/// Converts a PDB or DWARF image into the standard Breakpad symbol-store layout.
pub fn archive(input: &Path, store: &Path) -> Result<()> {
    let mut config = dump_syms::dumper::Config::with_output(dump_syms::dumper::Output::Store(
        store.to_path_buf(),
    ));
    config.num_jobs = 2;
    dump_syms::dumper::single_file(&config, input.to_str().context("non-UTF8 symbol path")?)
        .context("ARA-EXP-CRASH-005: converting debug symbols")
}

/// Symbolizes a dump with an explicit local symbol directory. Never contacts a server.
/// JSON panic/managed reports already contain their source stack and are also accepted.
pub fn symbolize(dump: &Path, store: &Path) -> Result<crate::CrashReport> {
    if dump.extension().is_some_and(|ext| ext == "managed") {
        let bytes = std::fs::read(dump)?;
        anyhow::ensure!(
            bytes.starts_with(b"ARAMCS1\0"),
            "ARA-EXP-CRASH-006: invalid managed crash record"
        );
        let mut cursor = std::io::Cursor::new(&bytes[8..]);
        use std::io::Read;
        let mut strings = Vec::new();
        for _ in 0..3 {
            let mut len = [0u8; 4];
            cursor.read_exact(&mut len)?;
            let len = u32::from_le_bytes(len) as usize;
            anyhow::ensure!(
                len <= 1024 * 1024,
                "ARA-EXP-CRASH-006: oversized managed crash field"
            );
            let mut text = vec![0u8; len];
            cursor.read_exact(&mut text)?;
            strings.push(String::from_utf8(text)?);
        }
        anyhow::ensure!(
            cursor.position() as usize == bytes.len() - 8,
            "ARA-EXP-CRASH-006: trailing managed crash bytes"
        );
        let report = crate::CrashReport::new(
            &strings[0],
            crate::CrashKind::Managed,
            strings[1].clone(),
            strings[2].clone(),
        );
        report.diagnostic().emit();
        return Ok(report);
    }
    if dump.extension().is_some_and(|ext| ext == "json") {
        if let Some(stem) = dump
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".crash.json"))
        {
            let native = dump.with_file_name(format!("{stem}.dmp"));
            if native.is_file() {
                return symbolize(&native, store);
            }
        }
        let report: crate::CrashReport = serde_json::from_slice(&std::fs::read(dump)?)?;
        anyhow::ensure!(
            report.format == "ara.crash/1",
            "ARA-EXP-CRASH-006: unknown crash report format"
        );
        report.diagnostic().emit();
        return Ok(report);
    }
    let md = minidump::Minidump::read_path(dump).context("ARA-EXP-CRASH-006: reading minidump")?;
    let symbolizer = breakpad_symbols::Symbolizer::new(StoreSupplier::new(store));
    let state = futures_executor::block_on(minidump_processor::process_minidump(&md, &symbolizer))?;
    let mut stack = Vec::new();
    state.print(&mut stack)?;
    let mut report = if let Ok(bytes) = std::fs::read(dump.with_extension("crash.json")) {
        serde_json::from_slice::<crate::CrashReport>(&bytes)?
    } else {
        crate::CrashReport::new(
            "unknown",
            crate::CrashKind::Native,
            "Native exception or fatal signal".into(),
            String::new(),
        )
    };
    report.stack = String::from_utf8(stack)?;
    report.diagnostic().emit();
    Ok(report)
}

/// Symbols from a local store: the standard `<name>/<id>/<name>.sym` layout, plus a
/// debug-ID-only match for a module the dump left unnamed (minidump-writer cannot
/// always read a macOS main executable's path; its LC_UUID is still recorded).
struct StoreSupplier {
    store: std::path::PathBuf,
    simple: breakpad_symbols::SimpleSymbolSupplier,
}

impl StoreSupplier {
    fn new(store: &Path) -> Self {
        Self {
            store: store.to_path_buf(),
            simple: breakpad_symbols::SimpleSymbolSupplier::new(vec![store.to_path_buf()]),
        }
    }
    /// The one `<any name>/<debug id>/*.sym` in the store for an unnamed module.
    fn by_debug_id(
        &self,
        module: &(dyn breakpad_symbols::Module + Sync),
    ) -> Option<std::path::PathBuf> {
        if !module.code_file().is_empty() {
            return None;
        }
        let id = breakpad_symbols::breakpad_sym_lookup(module)?.debug_id;
        std::fs::read_dir(&self.store)
            .ok()?
            .flatten()
            .map(|entry| entry.path().join(&id))
            .filter(|dir| dir.is_dir())
            .find_map(|dir| {
                std::fs::read_dir(dir)
                    .ok()?
                    .flatten()
                    .map(|entry| entry.path())
                    .find(|path| path.extension().is_some_and(|ext| ext == "sym"))
            })
    }
}

#[async_trait::async_trait]
impl breakpad_symbols::SymbolSupplier for StoreSupplier {
    async fn locate_symbols(
        &self,
        module: &(dyn breakpad_symbols::Module + Sync),
    ) -> Result<breakpad_symbols::LocateSymbolsResult, breakpad_symbols::SymbolError> {
        match self.simple.locate_symbols(module).await {
            Err(breakpad_symbols::SymbolError::NotFound) => {
                let path = self
                    .by_debug_id(module)
                    .ok_or(breakpad_symbols::SymbolError::NotFound)?;
                Ok(breakpad_symbols::LocateSymbolsResult {
                    symbols: breakpad_symbols::SymbolFile::from_file(&path)?,
                    extra_debug_info: None,
                })
            }
            located => located,
        }
    }
    async fn locate_file(
        &self,
        module: &(dyn breakpad_symbols::Module + Sync),
        file_kind: breakpad_symbols::FileKind,
    ) -> Result<std::path::PathBuf, breakpad_symbols::FileError> {
        self.simple.locate_file(module, file_kind).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use breakpad_symbols::{SimpleModule, SymbolError, SymbolSupplier};

    #[test]
    fn an_unnamed_module_finds_its_symbols_by_debug_id_alone() {
        let store = tempfile::tempdir().unwrap();
        let id = debugid::DebugId::from_breakpad("90DD4992B59E3FEE93FFF8F1A2D32F210").unwrap();
        let dir = store
            .path()
            .join("ara-player")
            .join(id.breakpad().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ara-player.sym"),
            format!(
                "MODULE mac arm64 {} ara-player
",
                id.breakpad()
            ),
        )
        .unwrap();
        let supplier = StoreSupplier::new(store.path());
        // minidump-writer names a macOS main executable it cannot read `<Unknown>`.
        let unnamed = SimpleModule::new("<Unknown>", id);
        let found = futures_executor::block_on(supplier.locate_symbols(&unnamed)).unwrap();
        assert_eq!(found.symbols.module_id, id.breakpad().to_string());
        // A named module is matched by name and ID only, as before.
        let named = SimpleModule {
            code_file: Some("libother.dylib".into()),
            ..SimpleModule::new("libother.dylib", id)
        };
        assert!(matches!(
            futures_executor::block_on(supplier.locate_symbols(&named)),
            Err(SymbolError::NotFound)
        ));
    }
}
