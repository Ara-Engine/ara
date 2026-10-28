//! Bounded external WAV fixtures and read-only authoring snapshots for native import proof.
use super::*;
use std::io::{Read, Write};

pub(super) struct Inputs {
    pub tone: PathBuf,
    pub other: PathBuf,
    pub large: PathBuf,
    pub undo: PathBuf,
    pub measurements: Value,
}
impl Inputs {
    pub fn new(output: &Path, project: &Path) -> ara_ui::Result<Self> {
        let output = dunce::canonicalize(output)?;
        let project = dunce::canonicalize(project)?;
        if output.starts_with(&project) {
            return Err("Native import inputs must be outside the scratch project".into());
        }
        let root = output.join("native-import-input");
        std::fs::create_dir_all(&root)?;
        let mut measurements = Vec::new();
        for (name, bytes, signal) in [
            ("native-tone.wav", 2048, 1),
            ("native-other.wav", 4096, 1),
            ("native-cancel.wav", 256 * 1024 * 1024, 1),
            ("native-undo.wav", 2048, 1),
        ] {
            let started = Instant::now();
            write_wav(&root.join(name), bytes, signal)?;
            measurements.push(json!({"file":name,"bytes":bytes+44,"writeMs":started.elapsed().as_secs_f64()*1000.}));
        }
        Ok(Self {
            tone: root.join("native-tone.wav"),
            other: root.join("native-other.wav"),
            large: root.join("native-cancel.wav"),
            undo: root.join("native-undo.wav"),
            measurements: json!(measurements),
        })
    }
    pub fn change_tone(&self, signal: u8) -> ara_ui::Result<()> {
        write_wav(&self.tone, 2048, signal)
    }
}
fn write_wav(path: &Path, bytes: u32, signal: u8) -> ara_ui::Result<()> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(b"RIFF")?;
    file.write_all(&(bytes + 36).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&48000_u32.to_le_bytes())?;
    file.write_all(&96000_u32.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    let chunk = [signal; 16 * 1024];
    let mut remaining = bytes as usize;
    while remaining > 0 {
        let count = remaining.min(chunk.len());
        file.write_all(&chunk[..count])?;
        remaining -= count;
    }
    file.flush()?;
    Ok(())
}
pub(super) fn selection(editor: &Editor, shell: &Shell) -> Value {
    shell.asset_import_selection(editor)
}
pub(super) fn snapshot(editor: &Editor, storage: &str) -> ara_ui::Result<Value> {
    let root = std::path::absolute(editor.host.root())?;
    let folder = root.join(storage);
    if !folder.starts_with(&root) || storage.split('/').any(|part| part == "..") {
        return Err("Import proof folder escapes its scratch root".into());
    }
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(&folder)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let mut file = std::fs::File::open(entry.path())?;
        let mut chunk = [0_u8; 16 * 1024];
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut bytes = 0_u64;
        loop {
            let count = file.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            for byte in &chunk[..count] {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        files.insert(
            entry.file_name().to_string_lossy().into_owned(),
            json!({"bytes":bytes,"fingerprint":format!("{hash:016x}")}),
        );
    }
    let docs: serde_json::Map<String, Value> = editor
        .host
        .store()
        .workspace()
        .docs()
        .map(|(path, doc)| {
            let canonical=ara_doc::write(doc,editor.host.store().schema());
            (
                path.to_owned(),
                json!({"id":doc.id,"kind":doc.kind,"bytes":canonical.len(),"digest":blake3::hash(canonical.as_bytes()).to_hex().to_string()}),
            )
        })
        .collect();
    let prefix = format!("{FOLDER}/");
    let identities: std::collections::BTreeMap<_, _> = editor
        .host
        .store()
        .workspace()
        .index()
        .imports()
        .filter_map(|(id, path)| path.strip_prefix(&prefix).map(|name| (name, id)))
        .collect();
    Ok(
        json!({"schemaVersion":1,"documentRepresentation":{"algorithm":"blake3","canonicalWriter":"ara_doc::write","keys":"exact workspace paths","source":"canonical documents retained in the isolated fixture"},
        "fileFingerprintAlgorithm":"fnv1a64","documents":docs,"files":files,"identities":identities}),
    )
}
