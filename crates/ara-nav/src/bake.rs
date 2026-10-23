//! Worker-side incremental tile baking and content-addressed storage.
use crate::{
    BakeArtifact, BakeInput, BakeSettings, BakeStats, Bounds, CacheKey, Geometry, NavError,
    OffMeshLink, TileCoord, TileData, native,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Synchronous worker-only baker. Use `BakeJob` to avoid blocking the editor.
pub struct Baker {
    cache: Option<PathBuf>,
}
impl Baker {
    /// Creates a baker with optional persistent content-addressed storage.
    pub fn new(cache: Option<PathBuf>) -> Self {
        Self { cache }
    }
    /// Rebuilds only tiles whose canonical inputs changed, including neighbor halos.
    pub fn bake(
        &self,
        input: &BakeInput,
        previous: Option<&BakeArtifact>,
    ) -> Result<BakeArtifact, NavError> {
        input.validate()?;
        let mut stats = BakeStats::default();
        if let Some(old) = previous {
            stats.removed = old
                .tiles
                .keys()
                .filter(|t| !input.tiles.contains(t))
                .count();
        }
        let mut tiles = BTreeMap::new();
        let prepared = prepare_geometry(input);
        for &coord in &input.tiles {
            let geometry =
                canonical_geometry(prepared.get(&coord).map(Vec::as_slice).unwrap_or_default());
            let (links, boxes) = tile_annotations(input, coord);
            let key = tile_key(&input.settings, coord, &geometry, &links, &boxes)?;
            if let Some(old) = previous
                .and_then(|p| p.tiles.get(&coord))
                .filter(|t| t.key == key)
            {
                tiles.insert(coord, old.clone());
                stats.reused += 1;
                continue;
            }
            if let Some(tile) = self.read_cache(key)? {
                tiles.insert(coord, tile);
                stats.cache_hits += 1;
                continue;
            }
            let bytes = native::bake(&input.settings, coord, &geometry, &links, &boxes)?;
            let tile = TileData {
                key,
                bytes: bytes.into(),
            };
            self.write_cache(&tile)?;
            tiles.insert(coord, tile);
            stats.rebuilt += 1;
        }
        let mut links = input.links.clone();
        links.sort_by_key(|l| l.id);
        Ok(BakeArtifact {
            settings: input.settings.clone(),
            tiles,
            links,
            stats,
        })
    }
    fn read_cache(&self, key: CacheKey) -> Result<Option<TileData>, NavError> {
        let Some(root) = &self.cache else {
            return Ok(None);
        };
        let bytes = match std::fs::read(root.join(key.to_string())) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if bytes.len() < 32 || blake3::hash(&bytes[32..]).as_bytes() != &bytes[..32] {
            return Ok(None); // A damaged cache entry is rebuilt; the last good mesh remains live.
        }
        Ok(Some(TileData {
            key,
            bytes: Arc::from(&bytes[32..]),
        }))
    }
    fn write_cache(&self, tile: &TileData) -> Result<(), NavError> {
        let Some(root) = &self.cache else {
            return Ok(());
        };
        std::fs::create_dir_all(root)?;
        let path = root.join(tile.key.to_string());
        let mut bytes = Vec::with_capacity(tile.bytes.len() + 32);
        bytes.extend_from_slice(blake3::hash(&tile.bytes).as_bytes());
        bytes.extend_from_slice(&tile.bytes);
        atomic_write(&path, &bytes)
    }
}

// Same-tile geometry is expanded into sorted triangles. Vertex buffer indices and
// producer iteration order therefore cannot change voxelization or its cache key.
type Triangle = ([[f32; 3]; 3], u8);
fn prepare_geometry(input: &BakeInput) -> BTreeMap<TileCoord, Vec<Triangle>> {
    let mut bins: BTreeMap<_, Vec<Triangle>> =
        input.tiles.iter().map(|&t| (t, Vec::new())).collect();
    let width = input.settings.tile_width();
    let margin = input.settings.halo();
    for (indices, &area) in input.geometry.triangles.iter().zip(&input.geometry.areas) {
        let mut tri = indices.map(|i| input.geometry.vertices[i as usize]);
        for p in &mut tri {
            for v in p {
                if *v == 0.0 {
                    *v = 0.0;
                }
            }
        }
        let mut tb = Bounds {
            min: tri[0],
            max: tri[0],
        };
        for p in tri {
            for (i, v) in p.into_iter().enumerate() {
                tb.min[i] = tb.min[i].min(v);
                tb.max[i] = tb.max[i].max(v);
            }
        }
        let first = (0..3)
            .min_by(|&a, &b| cmp_point(&tri[a], &tri[b]))
            .expect("triangle has three vertices");
        tri.rotate_left(first);
        // Only visit loaded X columns touched by this triangle's halo. The final
        // inclusive AABB test handles exact tile borders and stacked geometry.
        let x0 = (((tb.min[0] - margin - input.settings.origin[0]) / width).floor() as i32)
            .saturating_sub(1);
        let x1 = ((tb.max[0] + margin - input.settings.origin[0]) / width).ceil() as i32;
        for (&coord, triangles) in
            bins.range_mut(TileCoord { x: x0, z: i32::MIN }..=TileCoord { x: x1, z: i32::MAX })
        {
            if input
                .settings
                .tile_bounds(coord)
                .expanded(margin)
                .intersects(tb)
            {
                triangles.push((tri, area));
            }
        }
    }
    bins
}
fn canonical_geometry(triangles: &[Triangle]) -> Geometry {
    let mut triangles = triangles.to_vec();
    triangles.sort_by(|(a, aa), (b, ba)| {
        a.iter()
            .zip(b)
            .map(|(a, b)| cmp_point(a, b))
            .find(|o| !o.is_eq())
            .unwrap_or(aa.cmp(ba))
    });
    let mut geometry = Geometry::default();
    for (tri, area) in triangles {
        let offset = geometry.vertices.len() as u32;
        geometry.vertices.extend(tri);
        geometry.triangles.push([offset, offset + 1, offset + 2]);
        geometry.areas.push(area);
    }
    geometry
}
fn tile_annotations(input: &BakeInput, coord: TileCoord) -> (Vec<OffMeshLink>, Vec<Bounds>) {
    let bounds = input.settings.tile_bounds(coord);
    let halo = bounds.expanded(input.settings.halo());
    let mut links: Vec<_> = input
        .links
        .iter()
        .filter(|l| {
            l.enabled
                && l.start[0] >= bounds.min[0]
                && l.start[0] < bounds.max[0]
                && l.start[2] >= bounds.min[2]
                && l.start[2] < bounds.max[2]
        })
        .cloned()
        .collect();
    links.sort_by_key(|l| l.id);
    let mut obstacles: Vec<_> = input
        .obstacles
        .iter()
        .filter(|b| halo.intersects(b.bounds.expanded(input.settings.agent.radius)))
        .collect();
    obstacles.sort_by_key(|b| b.id);
    (links, obstacles.into_iter().map(|b| b.bounds).collect())
}
fn cmp_point(a: &[f32; 3], b: &[f32; 3]) -> std::cmp::Ordering {
    a.iter()
        .zip(b)
        .map(|(a, b)| a.total_cmp(b))
        .find(|o| !o.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}
fn tile_key(
    settings: &BakeSettings,
    coord: TileCoord,
    g: &Geometry,
    l: &[OffMeshLink],
    b: &[Bounds],
) -> Result<CacheKey, NavError> {
    let bytes = postcard::to_allocvec(&(settings, coord, g, l, b))
        .map_err(|e| NavError::new("encode", e.to_string()))?;
    let mut h = blake3::Hasher::new();
    h.update(b"ara-nav-v1-recast-b4554541-strict-fp");
    h.update(std::env::consts::ARCH.as_bytes());
    h.update(std::env::consts::OS.as_bytes());
    h.update(if cfg!(target_endian = "little") {
        b"le"
    } else {
        b"be"
    });
    h.update(&bytes);
    Ok(CacheKey(*h.finalize().as_bytes()))
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), NavError> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => {
            std::fs::remove_file(temp)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(temp);
            Err(e.into())
        }
    }
}
