//! The installed player PDB is an export input; other symbols stay private.
use crate::support::copy;
use anyhow::{Context, Result};
use std::path::Path;

pub(super) fn stage(binary: &str, pdb: &Path, engine: &Path, symbols: &Path) -> Result<()> {
    let name = pdb.file_name().context("PDB filename")?;
    copy(pdb, &symbols.join(name))?;
    if binary == "ara-player" {
        copy(pdb, &engine.join("ara_player.pdb"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Read};

    #[test]
    fn engine_zip_ships_only_the_player_pdb_with_the_original_pin_hash() {
        let scratch = ara_update::testing::scratch("release-player-symbols");
        // PID reuse can leave the previous invocation's engine directory here.
        fs::create_dir_all(scratch.join("engine")).expect("previous-run output");
        let fixture = tempfile::Builder::new()
            .prefix("payload-")
            .tempdir_in(scratch)
            .expect("fresh symbols fixture");
        let root = fixture.path();
        let engine = root.join("engine");
        let symbols = root.join("symbols");
        fs::create_dir(&engine).expect("engine");
        for binary in ["ara-player", "ara-editor", "ara"] {
            let source = root.join(format!("{}.pdb", binary.replace('-', "_")));
            fs::write(&source, format!("matching PDB for {binary}")).expect("PDB");
            stage(binary, &source, &engine, &symbols).expect("stage symbols");
            assert!(
                symbols
                    .join(source.file_name().expect("filename"))
                    .is_file()
            );
        }
        let zip = root.join("engine.zip");
        crate::archive::zip(&engine, &zip).expect("package");
        let mut archive = zip::ZipArchive::new(fs::File::open(zip).expect("zip")).expect("archive");
        assert_eq!(
            archive.len(),
            1,
            "only runtime-template symbols enter the engine"
        );
        let mut bytes = Vec::new();
        archive
            .by_name("ara_player.pdb")
            .expect("installed player PDB")
            .read_to_end(&mut bytes)
            .expect("bytes");
        let expected = blake3::hash(&fs::read(root.join("ara_player.pdb")).expect("source PDB"));
        assert_eq!(
            blake3::hash(&bytes),
            expected,
            "the editor's PDB pin must resolve to the original bytes"
        );
        assert!(archive.by_name("ara_editor.pdb").is_err());
        assert!(archive.by_name("ara.pdb").is_err());
    }
}
