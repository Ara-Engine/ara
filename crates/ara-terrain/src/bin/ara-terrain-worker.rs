//! One-shot out-of-process terrain cooker. The host spawns below normal priority,
//! polls completion without blocking, then publishes under the user's D20 policy.
use ara_terrain::{Terrain, TileCoord, cache::FileCache};
use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 6 {
        return Err("usage: ara-terrain-worker MANIFEST CACHE X Z OUTPUT".into());
    }
    let mut db = ara_schema::SchemaDb::new();
    ara_terrain::register(&mut db).map_err(|d| d.message)?;
    let read = ara_doc::read(&std::fs::read_to_string(&args[1])?, &args[1], &db, None);
    if let Some(d) = read.diags.iter().find(|d| d.is_error()) {
        return Err(d.message.clone().into());
    }
    let doc = read.doc.ok_or("manifest did not decode")?;
    let terrain = Terrain::from_document(&doc).map_err(|d| d.message)?;
    let coord = TileCoord {
        x: args[3].parse()?,
        z: args[4].parse()?,
    };
    let cache = FileCache::new(&args[2]);
    let tile = terrain.evaluate(coord, &cache).map_err(|d| d.message)?;
    let hash = cache.put(&tile).map_err(|d| d.message)?;
    let v = serde_json::json!({"coord":coord,"hash":hash});
    std::fs::write(Path::new(&args[5]), ara_json::to_canonical_string(&v))?;
    Ok(())
}
