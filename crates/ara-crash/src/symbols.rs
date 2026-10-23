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
    archive(&target, store)?;
    let symbol_name = format!("{}.sym", name.trim_end_matches(".pdb"));
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
        let report: crate::CrashReport = serde_json::from_slice(&std::fs::read(dump)?)?;
        anyhow::ensure!(
            report.format == "ara.crash/1",
            "ARA-EXP-CRASH-006: unknown crash report format"
        );
        report.diagnostic().emit();
        return Ok(report);
    }
    let md = minidump::Minidump::read_path(dump).context("ARA-EXP-CRASH-006: reading minidump")?;
    let supplier = breakpad_symbols::SimpleSymbolSupplier::new(vec![store.to_path_buf()]);
    let symbolizer = breakpad_symbols::Symbolizer::new(supplier);
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
