//! Isolate geometry with real Hub-project inputs and verified cached textures.
//! Never encodes, stores, rewrites sidecars or claims end-to-end import latency.
use ara_asset::{
    BlockEncoder, CacheBackend, CachedImport, Cancellation, CookPreset, Digest, Error, FileCache,
    ImportContext, ImportInput, ImportMetrics, ImportOutput, ImportServices, Importer, Result,
    Sidecar, TargetPlatform,
};
use ara_asset_format::TextureFormat;
use ara_importers::GltfImporter;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Instant,
};

struct TexturesOnly {
    cache: FileCache,
    keys: BTreeSet<Digest>,
}
impl CacheBackend for TexturesOnly {
    fn load(&self, key: Digest) -> Result<Option<CachedImport>> {
        if self.keys.contains(&key) {
            self.cache.load(key)
        } else {
            Ok(None)
        }
    }
    fn store(&self, _: Digest, _: &ImportOutput) -> Result<()> {
        Ok(())
    }
}
struct NoEncoding(String);
impl BlockEncoder for NoEncoding {
    fn backend(&self) -> &str {
        &self.0
    }
    fn encode(
        &self,
        _: TextureFormat,
        _: CookPreset,
        _: &[u32],
        _: &Cancellation,
    ) -> Result<Vec<u8>> {
        Err(Error::Invalid(
            "geometry replay encountered an uncached texture".into(),
        ))
    }
}
fn digest(text: &str) -> std::result::Result<Digest, Box<dyn std::error::Error>> {
    if text.len() != 64 {
        return Err("invalid digest length".into());
    }
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(Digest(bytes))
}
fn sources(root: &Path, paths: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sources(&entry.path(), paths)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gltf") || e.eq_ignore_ascii_case("glb"))
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}
fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let root = PathBuf::from(
        args.first()
            .ok_or("usage: geometry_replay PROJECT [threads] [path-substring]")?,
    )
    .canonicalize()?;
    let threads: u16 = args.get(1).map(|v| v.parse()).transpose()?.unwrap_or(4);
    let cache_root = root.join(".ara/cache");
    if !cache_root.join("actions").is_dir() {
        return Err("a completed project cache is required".into());
    }
    let cache = FileCache::at(&cache_root)?;
    let intermediate = cache_root.join("intermediates");
    let mut keys = BTreeSet::new();
    for file in std::fs::read_dir(intermediate.join("actions"))? {
        let file = file?;
        let Some(name) = file
            .file_name()
            .to_str()
            .map(str::to_owned)
            .filter(|s| s.len() == 64)
        else {
            continue;
        };
        let entry: serde_json::Value = serde_json::from_slice(&std::fs::read(file.path())?)?;
        if entry["artifacts"]
            .as_array()
            .is_some_and(|a| a.len() == 1 && a[0]["name"] == "texture.ktx2")
        {
            keys.insert(digest(&name)?);
        }
    }
    let memo = TexturesOnly {
        cache: FileCache::at(intermediate)?,
        keys,
    };
    eprintln!("verified texture entries: {}", memo.keys.len());
    let hints = FileCache::at(cache_root.join("encoder-hints"))?;
    let mut identities = BTreeSet::new();
    for file in std::fs::read_dir(hints.root().join("actions"))? {
        let file = file?;
        let Some(name) = file
            .file_name()
            .to_str()
            .map(str::to_owned)
            .filter(|s| s.len() == 64)
        else {
            continue;
        };
        let hit = hints.load(digest(&name)?)?.ok_or("invalid encoder hint")?;
        if hit.output.artifacts.len() != 1 || hit.output.artifacts[0].name != "identity.json" {
            return Err("unexpected encoder hint shape".into());
        }
        identities.insert(serde_json::from_slice::<String>(
            &hit.output.artifacts[0].bytes,
        )?);
    }
    if identities.len() != 1 {
        return Err("exactly one verified encoder identity is required".into());
    }
    let encoder = NoEncoding(identities.into_iter().next().ok_or("encoder identity")?);
    let mut db = ara_schema::SchemaDb::new();
    ara_asset::register_schema(&mut db);
    let registry = ara_importers::registry()?;
    let importer = GltfImporter;
    let mut paths = Vec::new();
    sources(&root.join("assets"), &mut paths)?;
    paths.sort();
    let mut rows = Vec::new();
    for path in paths {
        let relative = path
            .strip_prefix(&root)?
            .to_str()
            .ok_or("non-Unicode source")?
            .replace('\\', "/");
        if args.get(2).is_some_and(|filter| !relative.contains(filter)) {
            continue;
        }
        let sidecar_path = path.with_file_name(format!(
            "{}.import.json",
            path.file_name().ok_or("source name")?.to_string_lossy()
        ));
        if !sidecar_path.is_file() {
            continue;
        }
        eprintln!("replay {relative}");
        let sidecar =
            Sidecar::read(&std::fs::read_to_string(sidecar_path)?, &relative, &db)?.settings()?;
        if sidecar.importer != importer.id()
            || sidecar.version != importer.version()
            || !sidecar.dependencies.is_empty()
        {
            return Err(format!("unsupported/stale sidecar: {relative}").into());
        }
        let source = std::fs::read(&path)?;
        let dependencies = importer
            .dependencies_with_settings(&source, &sidecar.settings)?
            .into_iter()
            .map(|uri| {
                let bytes = std::fs::read(path.parent().ok_or("source parent")?.join(&uri))?;
                Ok((uri, bytes))
            })
            .collect::<std::result::Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;
        let input = ImportInput {
            preset: sidecar.preset.unwrap_or_default(),
            thread_budget: threads,
            source_path: relative.clone(),
            source,
            importer: importer.id().into(),
            version: importer.version().into(),
            settings: sidecar.settings,
            target: TargetPlatform::Windows,
            dependencies,
            asset_dependencies: BTreeMap::new(),
        };
        let key = input.key_with_backend(Some(encoder.backend()))?;
        let expected = cache
            .load(key)?
            .ok_or_else(|| format!("no matching production action for {relative}"))?;
        let metrics = ImportMetrics::default();
        let cancellation = Cancellation::default();
        let began = Instant::now();
        let output = registry.import_with_services(
            &input,
            &mut ImportContext {
                metrics: Some(&metrics),
                cancellation: &cancellation,
                progress: &mut |_| {},
            },
            ImportServices {
                memo: Some(&memo),
                encoder: Some(&encoder),
            },
        )?;
        let cook_ms = began.elapsed().as_secs_f64() * 1000.;
        if expected.output != output {
            return Err(format!("artifact bytes changed: {relative}").into());
        }
        let stages = metrics.snapshot();
        if stages.texture_cooks != 0 || stages.decode_ms != 0. || stages.mips_ms != 0. {
            return Err("textures were not completely reused".into());
        }
        rows.push(
            serde_json::json!({"source":relative,"key":key.to_string(),"cookMs":cook_ms,
            "artifacts":output.artifacts.len(),"stages":stages}),
        );
    }
    if rows.is_empty() {
        return Err("no model sources selected".into());
    }
    println!(
        "{}",
        ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::json!({
        "definition":"Pure geometry replay: real inputs, verified cached textures, no encoding/stores; not end-to-end latency",
        "project":root,"threads":threads,"models":rows.len(),"assets":rows})))
    );
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
