//! Deterministic worker-side sample content (hills, stratified cliffs and vegetation).
use anyhow::{Result, anyhow};
use ara_foliage::{Bounds, Instance, Rule, Species, SurfaceSample};
use ara_schema::AssetRef;
use ara_terrain::{Header, Tile, TileCoord};

/// Immutable terrain extraction; each row includes the shared outer border.
pub struct LandscapeData {
    /// Terrain configuration.
    pub header: Header,
    /// Packed samples: height bits, packed four-channel UNORM weights, hole flag.
    pub samples: Vec<[u32; 3]>,
    /// Persistent instance input, extracted on a worker.
    pub instances: Vec<Instance>,
    /// Exact sample width in samples.
    pub side: u32,
}

/// Sample landscape height in metres; no clock or hidden RNG input.
pub fn height(x: f32, z: f32) -> f32 {
    let rolling =
        (x * 0.0031).sin() * 34. + (z * 0.0048).cos() * 24. + ((x + z) * 0.008).sin() * 8.;
    let ridge = ((x - 1080.) * 0.007).tanh() * 43.;
    let cliff = ((z - 980. + (x * 0.006).sin() * 38.) * 0.07).tanh() * 26.;
    85. + rolling + ridge + cliff + (x * 0.035).sin() * (z * 0.027).cos() * 1.8
}
/// Analytic sampling of the sample landscape for deterministic scatter workers.
pub fn surface(p: [f32; 2]) -> Option<SurfaceSample> {
    let h = height(p[0], p[1]);
    let dx = (height(p[0] + 1., p[1]) - height(p[0] - 1., p[1])) * 0.5;
    let dz = (height(p[0], p[1] + 1.) - height(p[0], p[1] - 1.)) * 0.5;
    let normal = glam::Vec3::new(-dx, 1., -dz).normalize();
    let rock = ((1. - normal.y) * 2.7).clamp(0., 1.);
    Some(SurfaceSample {
        height: h,
        normal: normal.to_array(),
        weights: [1. - rock, rock, 0., 0.],
    })
}
/// Independent source tile generation. Shared-edge samples are global functions.
pub fn tile(header: &Header, c: TileCoord) -> Result<Tile> {
    let mut tile = Tile::flat(header.cells).map_err(|d| anyhow!(d.message))?;
    let side = header.cells + 1;
    for z in 0..side {
        for x in 0..side {
            let p = [
                header.origin[0] + (c.x * header.cells as i32 + x as i32) as f32 * header.spacing,
                header.origin[1] + (c.z * header.cells as i32 + z as i32) as f32 * header.spacing,
            ];
            let s = surface(p).ok_or_else(|| anyhow!("sample surface missing"))?;
            let rock = (s.weights[1] * 255.).round() as u8;
            let i = (z * side + x) as usize;
            tile.heights[i] = s.height;
            tile.weights[i] = [255 - rock, rock, 0, 0];
        }
    }
    Ok(tile)
}
/// Sample placement rules. Source models are procedural CC0 example geometry.
pub fn rules() -> Vec<Rule> {
    [
        (Species::Grass, 3., 0.09, [0.6, 1.45]),
        (Species::Tree, 12., 0.0018, [0.85, 1.8]),
        (Species::Rock, 14., 0.0008, [0.8, 3.]),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (species, cell_size, density, scale))| Rule {
        name: format!("Landscape{i}"),
        species,
        asset: AssetRef(format!("@sample/species{i}.mesh#me_sample0{i}")),
        seed: 42 + i as u64,
        cell_size,
        density,
        height: [-100., 400.],
        slope: [0., if i == 2 { 85. } else { 42. }],
        channel: 0,
        min_weight: 0.,
        noise: 0.15,
        scale,
        enabled: true,
    })
    .collect()
}
/// Builds a 4096 m square, saving binary tiles and a canonical terrain manifest when
/// `cache_root` is supplied. This is offline/worker work, never a render-thread call.
pub fn landscape(cache_root: Option<&std::path::Path>) -> Result<LandscapeData> {
    let header = Header {
        origin: [0., 0.],
        spacing: 2.,
        cells: 32,
        min: TileCoord { x: 0, z: 0 },
        max: TileCoord { x: 64, z: 64 },
        materials: vec![],
    };
    let side = 2049;
    let mut samples = vec![[0u32; 3]; side * side];
    let mut instances = Vec::new();
    let cache = cache_root.map(ara_terrain::cache::FileCache::new);
    let mut terrain = ara_terrain::Terrain::new(
        ara_doc::AssetId::parse("tr_land0001").map_err(|e| anyhow!(e))?,
        header.clone(),
    )
    .map_err(|d| anyhow!(d.message))?;
    let rules = rules();
    for z in 0..64 {
        for x in 0..64 {
            let coord = TileCoord { x, z };
            let t = tile(&header, coord)?;
            for tz in 0..33 {
                for tx in 0..33 {
                    let src = tz * 33 + tx;
                    let dst = (z as usize * 32 + tz) * side + x as usize * 32 + tx;
                    samples[dst] = [
                        t.heights[src].to_bits(),
                        u32::from_le_bytes(t.weights[src]),
                        u32::from(t.holes[src]),
                    ];
                }
            }
            if let Some(cache) = &cache {
                let hash = cache.put(&t).map_err(|d| anyhow!(d.message))?;
                terrain.tiles.insert(
                    format!("t_{x:02}{z:02}"),
                    ara_terrain::TileRecord {
                        coord,
                        base: Some(hash),
                        strokes: vec![],
                    },
                );
            }
            let bounds = Bounds {
                min: [x as f32 * 64., z as f32 * 64.],
                max: [(x + 1) as f32 * 64., (z + 1) as f32 * 64.],
            };
            for (rule_id, rule) in rules.iter().enumerate() {
                instances.extend(
                    ara_foliage::scatter(rule_id as u64, rule, bounds, &surface, &[])
                        .map_err(|d| anyhow!(d.message))?,
                );
            }
        }
    }
    if let Some(root) = cache_root {
        let mut db = ara_schema::SchemaDb::new();
        ara_terrain::register(&mut db).map_err(|d| anyhow!(d.message))?;
        std::fs::create_dir_all(root)?;
        std::fs::write(
            root.join("landscape.terrain"),
            ara_doc::write(&terrain.to_document().map_err(|d| anyhow!(d.message))?, &db),
        )?;
    }
    Ok(LandscapeData {
        header,
        samples,
        instances,
        side: side as u32,
    })
}
