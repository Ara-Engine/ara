//! dx's x64 text thunk clobbers RAX, the input/output register of __chkstk.
//! Rewrite only its generated COFF stub before linking, never loaded code.
use anyhow::{Context, Result, ensure};
use object::{Object, ObjectSection, ObjectSymbol};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub(super) fn prepare(path: &Path) -> Result<PathBuf> {
    let input = std::fs::read(path)?;
    let Some(bytes) = preserve_stack_probe(&input)? else {
        return Ok(path.to_owned());
    };
    let output = path.with_file_name(format!("ara-stub-{}.o", std::process::id()));
    std::fs::write(&output, bytes)?;
    Ok(output)
}

fn preserve_stack_probe(input: &[u8]) -> Result<Option<Vec<u8>>> {
    let file = object::File::parse(input)?;
    ensure!(
        file.format() == object::BinaryFormat::Coff
            && file.architecture() == object::Architecture::X86_64,
        "ARA7600: unsupported dx Windows stub format"
    );
    let Some(probe) = file.symbols().find(|s| s.name() == Ok("__chkstk")) else {
        return Ok(None);
    };
    let section = probe
        .section_index()
        .context("__chkstk stub has no section")?;
    let original = file.section_by_index(section)?.data()?;
    let offset = usize::try_from(probe.address())?;
    let code = original
        .get(offset..offset + 12)
        .context("truncated dx __chkstk thunk")?;
    ensure!(
        code[..2] == [0x48, 0xb8] && code[10..] == [0xff, 0xe0],
        "ARA7600: dx changed its __chkstk thunk; needs a driver update"
    );
    // RIP-relative indirect jump preserves every register, including stack size in
    // RAX. The embedded address is already ASLR-adjusted by dx for this host PID.
    let mut jump = vec![0xff, 0x25, 0, 0, 0, 0];
    jump.extend_from_slice(&code[2..10]);
    let mut output =
        object::write::Object::new(file.format(), file.architecture(), file.endianness());
    let mut sections = HashMap::new();
    let mut probe_offset = 0;
    for source in file.sections() {
        ensure!(
            source.relocations().next().is_none(),
            "ARA7600: dx stub unexpectedly contains relocations"
        );
        let id = output.add_section(Vec::new(), source.name_bytes()?.to_vec(), source.kind());
        sections.insert(source.index(), id);
        let target = output.section_mut(id);
        target.flags = source.flags();
        if source.kind().is_bss() {
            target.append_bss(source.size(), source.align());
        } else {
            target.set_data(source.data()?.to_vec(), source.align());
            if source.index() == section {
                probe_offset = target.append_data(&jump, 8);
            }
        }
    }
    for symbol in file.symbols() {
        if matches!(
            symbol.kind(),
            object::SymbolKind::Section | object::SymbolKind::File
        ) {
            continue;
        }
        let target = match symbol.section() {
            object::SymbolSection::None => object::write::SymbolSection::None,
            object::SymbolSection::Undefined => object::write::SymbolSection::Undefined,
            object::SymbolSection::Absolute => object::write::SymbolSection::Absolute,
            object::SymbolSection::Section(index) => {
                object::write::SymbolSection::Section(sections[&index])
            }
            _ => anyhow::bail!("ARA7600: unsupported dx stub symbol section"),
        };
        let fixed = symbol.index() == probe.index();
        output.add_symbol(object::write::Symbol {
            name: symbol.name_bytes()?.to_vec(),
            value: if fixed {
                probe_offset
            } else {
                symbol.address()
            },
            size: if fixed {
                jump.len() as u64
            } else {
                symbol.size()
            },
            kind: symbol.kind(),
            scope: symbol.scope(),
            weak: symbol.is_weak(),
            section: target,
            flags: object::SymbolFlags::None,
        });
    }
    Ok(Some(output.write()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stack_probe_preserves_rax_and_other_stub_addresses() {
        let mut input = object::write::Object::new(
            object::BinaryFormat::Coff,
            object::Architecture::X86_64,
            object::Endianness::Little,
        );
        let text = input.section_id(object::write::StandardSection::Text);
        let address = 0x1234_5678_90ab_cdef_u64;
        let mut bad = vec![0x48, 0xb8];
        bad.extend_from_slice(&address.to_le_bytes());
        bad.extend_from_slice(&[0xff, 0xe0]);
        input.append_section_data(text, &bad, 8);
        for (name, offset) in [("__chkstk", 0), ("other", 0)] {
            input.add_symbol(object::write::Symbol {
                name: name.as_bytes().to_vec(),
                value: offset,
                size: 12,
                kind: object::SymbolKind::Text,
                scope: object::SymbolScope::Linkage,
                weak: false,
                section: object::write::SymbolSection::Section(text),
                flags: object::SymbolFlags::None,
            });
        }
        let fixed = preserve_stack_probe(&input.write().unwrap())
            .unwrap()
            .unwrap();
        let file = object::File::parse(fixed.as_slice()).unwrap();
        let symbol = file.symbols().find(|s| s.name() == Ok("__chkstk")).unwrap();
        let code = file
            .section_by_index(symbol.section_index().unwrap())
            .unwrap()
            .data()
            .unwrap();
        let start = symbol.address() as usize;
        assert_eq!(&code[start..start + 6], &[0xff, 0x25, 0, 0, 0, 0]);
        assert_eq!(&code[start + 6..start + 14], &address.to_le_bytes());
        assert_eq!(
            file.symbols()
                .find(|s| s.name() == Ok("other"))
                .unwrap()
                .address(),
            0
        );
    }
}
