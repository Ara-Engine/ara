#![allow(clippy::unwrap_used)]
//! The Valley, the integration level: a 64 m sculpted terrain with foliage, streamed
//! as a 4-cell world (terrain chunks, props, lamps and baked acoustics per cell), the
//! motion-matched Fox, a guard NPC (behaviour tree on nav v2 + perception), the
//! campfire (VFX, crackle, acoustic source), interactive music and lamps.
//!
//! [`valley_documents`] authors every file through typed commands (terrain, foliage,
//! behaviour, sidecars) or the canonical writer; `ARA_UPDATE_DEMO=1` rewrites them
//! under `ara-player/demo`. The other tests play the level in-process: streaming and
//! grounding across cells, streamed navigation, the guard's brain, determinism, exact
//! checkpoints and replay. `ara-player`'s `valley` test plays it networked over MCP.
use ara_asset::{CachedImport, Cancellation, ImportContext, ImportInput, TargetPlatform};
use ara_cmd::{Batch, Registry, Session as CmdSession};
use ara_doc::{AssetId, Document, Record, RecordId, Workspace};
use ara_input::{ActionValue, Injection, PlayerId};
use ara_runtime::{Runtime, RuntimeConfig, assets::MemoryAssets, terrain::TerrainData};
use ara_runtime_world::WorldStreaming;
use ara_schema::SchemaDb;
use ara_sim::StepInput;
use ara_terrain::{Tile, TileCoord};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

fn demo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ara-player/demo")
}

/// Terrain: 2×2 tiles of 32 m (one per world cell), origin-anchored for the landscape.
const TILE: f32 = 32.0;
const SCENE: (&str, &str) = ("levels/valley.scene", "sc_valley1");
const TERRAIN: (&str, &str) = ("terrain/valley.terrain", "tr_valley1");
const FOLIAGE: (&str, &str) = ("terrain/valley.foliage", "fo_valley1");
const WORLD: (&str, &str) = ("world/valley.world", "wd_valley1");
const GUARD: (&str, &str) = ("ai/guard.behavior.graph", "gr_guard1");
const MUSIC: (&str, &str) = ("audio/valley.music", "mu_valley1");
const FOX: &str = "@assets/fox/Fox.glb#as_fox";
const ROCK: &str = "@assets/props/rock.glb#as_rock";

/// Where the player starts (in cell 0,0, 12 m from cell 1,0).
const START: [f32; 2] = [20.0, 14.0];

fn reference((path, id): (&str, &str)) -> String {
    format!("@{path}#{id}")
}

/// The engine schema world and command registry (what every host registers).
fn engine() -> (SchemaDb, Registry) {
    let mut db = ara_runtime::schema(&[]).unwrap();
    let mut registry = Registry::builtin(&mut db);
    ara_runtime::engine::register_commands(&mut registry, &mut db);
    (db, registry)
}

/// Applies `calls` to `doc` as one human gesture.
fn apply(
    store: &mut ara_ops::Store,
    registry: &Registry,
    doc: &str,
    calls: Vec<(&'static str, Value)>,
) {
    registry
        .apply(store, &Batch::new(doc, calls), &CmdSession::human())
        .unwrap_or_else(|d| panic!("{doc}: {d:#?}"));
}

/// Base tiles are never imported here: the valley is sculpted by strokes.
struct NoBases;
impl ara_terrain::cache::TileSource for NoBases {
    fn load(&self, hash: &str) -> Result<Tile, ara_diag::Diagnostic> {
        Err(ara_terrain::invalid(format!("no base {hash}")))
    }
}

/// Every tile of the terrain document, evaluated.
fn evaluate(doc: &Document) -> TerrainData {
    let terrain = ara_terrain::Terrain::from_document(doc).unwrap();
    let mut tiles = BTreeMap::new();
    for z in terrain.header.min.z..terrain.header.max.z {
        for x in terrain.header.min.x..terrain.header.max.x {
            let coord = TileCoord { x, z };
            tiles.insert(coord, terrain.evaluate(coord, &NoBases).unwrap());
        }
    }
    TerrainData {
        header: terrain.header.clone(),
        tiles,
        foliage: Vec::new(),
    }
}

/// Sculpts the valley: hills on the far sides, a flat camp around (16, 16) on soil,
/// rock on the hilltops.
fn terrain(store: &mut ara_ops::Store, registry: &Registry) {
    let dab = |x: f32, z: f32, radius: f32, amount: f32| json!({"center": [x, z], "radius": radius, "amount": amount});
    apply(
        store,
        registry,
        TERRAIN.1,
        vec![
            (
                "terrain.create",
                json!({"grid": {"origin": [0, 0], "spacing": 1, "cells": 32,
                    "min": {"x": 0, "z": 0}, "max": {"x": 2, "z": 2}, "materials": []}}),
            ),
            (
                "terrain.layer.add",
                json!({"id": "l_sculpt", "layer": {"name": "Sculpt", "priority": 0, "enabled": true, "opacity": 1}}),
            ),
            (
                "terrain.raise",
                json!({"layer": "l_sculpt", "dabs": [
                    dab(52.0, 50.0, 16.0, 7.0), dab(46.0, 58.0, 10.0, 3.0),
                    dab(6.0, 46.0, 12.0, 5.0), dab(12.0, 58.0, 10.0, 4.0),
                    dab(54.0, 10.0, 9.0, 2.5), dab(30.0, 50.0, 8.0, 1.5)]}),
            ),
            (
                "terrain.paint",
                json!({"layer": "l_sculpt", "channel": 1, "dabs": [
                    dab(52.0, 50.0, 9.0, 0.9), dab(8.0, 48.0, 8.0, 0.8), dab(54.0, 10.0, 5.0, 0.7)]}),
            ),
            (
                "terrain.paint",
                json!({"layer": "l_sculpt", "channel": 2, "dabs": [dab(16.0, 16.0, 6.0, 0.8)]}),
            ),
        ],
    );
}

/// Grass on the meadows, trees on the lower slopes, rocks on the rocky hilltops; none
/// in the camp.
fn foliage(store: &mut ara_ops::Store, registry: &Registry) {
    let rule = |name: &str,
                species: &str,
                seed: u64,
                cell: f32,
                density: f32,
                height: [f32; 2],
                slope: [f32; 2],
                channel: u32,
                scale: [f32; 2]| {
        json!({"name": name, "species": species, "asset": ROCK, "seed": seed, "cellSize": cell,
            "density": density, "height": height, "slope": slope, "channel": channel,
            "minWeight": 0.4, "noise": 0.5, "scale": scale, "enabled": true})
    };
    apply(
        store,
        registry,
        FOLIAGE.1,
        vec![
            (
                "foliage.create",
                json!({"header": {"terrain": reference(TERRAIN)}}),
            ),
            (
                "foliage.rule.set",
                json!({"id": "r_grass", "rule": rule("Grass", "grass", 11, 1.0, 0.3, [-2.0, 5.0], [0.0, 30.0], 0, [0.25, 0.45])}),
            ),
            (
                "foliage.rule.set",
                json!({"id": "r_trees", "rule": rule("Trees", "tree", 12, 4.0, 0.015, [0.3, 6.0], [0.0, 28.0], 0, [0.55, 0.9])}),
            ),
            (
                "foliage.rule.set",
                json!({"id": "r_rocks", "rule": rule("Rocks", "rock", 13, 3.0, 0.05, [1.0, 9.0], [5.0, 60.0], 1, [0.4, 1.0])}),
            ),
            (
                "foliage.exclude",
                json!({"zone": {"center": [16, 16], "radius": 7}}),
            ),
            (
                "foliage.exclude",
                json!({"zone": {"center": [START[0], START[1]], "radius": 3}}),
            ),
        ],
    );
}

/// The guard: patrols its route, investigates noises, chases the player on sight and
/// searches where it was last seen.
const GUARD_TEXT: &str = include_str!("../../ara-ai/behaviors/guard.behavior");

fn guard_graph() -> Document {
    let behavior = ara_ai::dsl::parse(GUARD_TEXT, GUARD.1).unwrap();
    ara_ai::projection::to_graph(&behavior, Default::default())
        .unwrap()
        .to_document()
        .unwrap()
}

/// An entity record: optional name plus components.
fn entity(name: &str, components: Value) -> Record {
    let mut record = Record::new();
    record.insert("name".into(), json!(name));
    for (k, v) in components.as_object().unwrap() {
        record.insert(k.clone(), v.clone());
    }
    record
}

/// A document of `records` in order (order keys assigned here).
fn document(kind: &str, id: &str, records: Vec<(&str, Record)>) -> Document {
    let mut doc = Document::new(kind, AssetId::parse(id).unwrap());
    let mut last: Option<ara_doc::OrderKey> = None;
    for (rid, mut record) in records {
        if kind == "scene" {
            let key = ara_doc::order::key_between(last.as_ref(), None).unwrap();
            record.insert("order".into(), json!(key.as_str()));
            last = Some(key);
        }
        doc.insert_unchecked(RecordId::parse(rid).unwrap(), record);
    }
    doc
}

/// Feet height at world X/Z.
fn ground(data: &TerrainData, x: f32, z: f32) -> f32 {
    data.height(x, z).unwrap()
}

fn rock(data: &TerrainData, x: f32, z: f32, size: f32, physics: u64) -> Record {
    entity(
        "Rock",
        json!({
            "Transform": {"position": [x, ground(data, x, z) + 0.3 * size, z], "rotation": [0, (x * 37.0 + z * 11.0) % 360.0, 0], "scale": [size, size * 0.8, size]},
            "MeshRenderer": {"model": ROCK, "roughness": 0.9},
            "RigidBody": {},
            "Collider": {"halfExtents": [0.45 * size, 0.35 * size, 0.45 * size]},
            "PhysicsId": {"value": physics},
            "AcousticSurface": {"preset": "rock"},
        }),
    )
}

fn crate_box(
    data: &TerrainData,
    x: f32,
    z: f32,
    size: f32,
    color: [f32; 3],
    physics: u64,
) -> Record {
    entity(
        "Crate",
        json!({
            "Transform": {"position": [x, ground(data, x, z) + size * 0.5 + 0.02, z]},
            "MeshRenderer": {"size": [size, size, size], "color": [color[0], color[1], color[2], 1], "roughness": 0.7},
            "RigidBody": {"motion": "dynamic", "mass": 12},
            "Collider": {"halfExtents": [size * 0.5, size * 0.5, size * 0.5], "friction": 0.7},
            "PhysicsId": {"value": physics},
            "AcousticSurface": {"preset": "wood"},
        }),
    )
}

fn lamp(data: &TerrainData, x: f32, z: f32, color: [f32; 3]) -> Vec<Record> {
    let h = ground(data, x, z);
    vec![
        entity(
            "Lamp post",
            json!({"Transform": {"position": [x, h + 1.1, z]},
                "MeshRenderer": {"size": [0.12, 2.2, 0.12], "color": [0.15, 0.13, 0.12, 1], "roughness": 0.8}}),
        ),
        entity(
            "Lamp",
            json!({"Transform": {"position": [x, h + 2.35, z]},
                "Light": {"color": [color[0], color[1], color[2], 1], "intensity": 9000, "range": 10},
                "MeshRenderer": {"shape": "sphere", "size": [0.25, 0.25, 0.25], "color": [color[0], color[1], color[2], 1], "emissive": [color[0] * 900.0, color[1] * 900.0, color[2] * 900.0]}}),
        ),
    ]
}

/// One world cell: its terrain chunk (collision, navigation, landscape pages) plus
/// props and lamps.
fn cell(data: &TerrainData, x: i32, z: i32) -> Document {
    let tag = format!("c{x}{z}");
    let id = |what: &str| format!("e_{tag}{what}");
    let (x0, z0) = (x as f32 * TILE, z as f32 * TILE);
    let at = |u: f32, v: f32| (x0 + u * TILE, z0 + v * TILE);
    let base = 100 + (x * 2 + z) as u64 * 20;
    let mut records: Vec<(String, Record)> = Vec::new();
    if (x, z) != (0, 0) {
        // The camp's chunk lives in the level (always resident).
        records.push((
            id("ground"),
            entity(
                "Terrain chunk",
                json!({"TerrainChunk": {"x": x, "z": z}, "AcousticSurface": {"preset": "gravel"}}),
            ),
        ));
    }
    let rocks: &[(f32, f32, f32)] = match (x, z) {
        (0, 0) => &[(0.12, 0.85, 1.6), (0.88, 0.1, 1.2), (0.1, 0.12, 1.0)],
        (1, 0) => &[(0.3, 0.3, 1.4), (0.62, 0.7, 2.0), (0.8, 0.25, 1.1)],
        (0, 1) => &[(0.4, 0.2, 1.3), (0.7, 0.55, 1.7), (0.25, 0.75, 1.0)],
        _ => &[(0.3, 0.6, 1.5), (0.55, 0.25, 1.2), (0.85, 0.8, 2.2)],
    };
    for (i, &(u, v, size)) in rocks.iter().enumerate() {
        let (wx, wz) = at(u, v);
        records.push((
            id(&format!("rock{i}")),
            rock(data, wx, wz, size, base + i as u64),
        ));
    }
    if (x, z) == (1, 0) {
        // A supply cache east of the camp, across the cell border.
        for (i, (u, v)) in [(0.18, 0.42), (0.21, 0.47), (0.19, 0.52)]
            .iter()
            .enumerate()
        {
            let (wx, wz) = at(*u, *v);
            records.push((
                id(&format!("crate{i}")),
                crate_box(data, wx, wz, 0.7, [0.75, 0.4, 0.15], base + 10 + i as u64),
            ));
        }
    }
    if (x, z) == (1, 1) {
        // A watchtower with a beacon on the hill.
        let (wx, wz) = at(0.45, 0.4);
        let h = ground(data, wx, wz);
        records.push((
            id("tower"),
            entity(
                "Watchtower",
                json!({"Transform": {"position": [wx, h + 2.5, wz]},
                    "MeshRenderer": {"size": [1.6, 5, 1.6], "color": [0.36, 0.3, 0.24, 1], "roughness": 0.85},
                    "RigidBody": {}, "Collider": {"halfExtents": [0.8, 2.5, 0.8]},
                    "PhysicsId": {"value": base + 15}, "AcousticSurface": {"preset": "wood"}}),
            ),
        ));
        records.push((
            id("beacon"),
            entity(
                "Beacon",
                json!({"Transform": {"position": [wx, h + 5.6, wz]},
                    "Light": {"color": [1, 0.35, 0.15, 1], "intensity": 30000, "range": 22}}),
            ),
        ));
    }
    if (x, z) == (0, 1) {
        // Ruined walls.
        for (i, (u, v, w, d)) in [(0.5, 0.3, 6.0, 0.6), (0.7, 0.42, 0.6, 4.0)]
            .iter()
            .enumerate()
        {
            let (wx, wz) = at(*u, *v);
            let h = ground(data, wx, wz);
            records.push((
                id(&format!("wall{i}")),
                entity(
                    "Ruined wall",
                    json!({"Transform": {"position": [wx, h + 0.9, wz]},
                        "MeshRenderer": {"size": [w, 1.8, d], "color": [0.42, 0.41, 0.4, 1], "roughness": 0.9},
                        "RigidBody": {}, "Collider": {"halfExtents": [w * 0.5, 0.9, d * 0.5]},
                        "PhysicsId": {"value": base + 16 + i as u64}, "AcousticSurface": {"preset": "brick"}}),
                ),
            ));
        }
    }
    let (lx, lz) = at(0.5, 0.5);
    for (i, record) in lamp(data, lx, lz, [1.0, 0.75, 0.45])
        .into_iter()
        .enumerate()
    {
        records.push((id(&format!("lamp{i}")), record));
    }
    document(
        "scene",
        &format!("sc_cell{x}{z}"),
        records
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect(),
    )
}

/// Baked reverb probes for one cell (geometric baker over the terrain and the cell's
/// rocks and walls), as the `ARAACU01` blob the `ara.acoustics` importer publishes.
fn acoustics(data: &TerrainData, cell: &Document, x: i32, z: i32) -> Vec<u8> {
    use ara_acoustics::{
        MaterialPreset,
        bake::{AcousticCell, CellKey, GeometricBaker, ReverbBaker, floor_probes},
        geometry::MeshScene,
    };
    let mut scene = MeshScene::new();
    for (coord, tile) in &data.tiles {
        let mesh = tile.heightfield(&data.header, *coord).mesh();
        scene
            .add_mesh(
                &mesh.vertices,
                &mesh.triangles,
                1,
                "terrain",
                MaterialPreset::Gravel.material(),
            )
            .unwrap();
    }
    for (id, record) in cell.records() {
        if let (Some(t), Some(c)) = (record.get("Transform"), record.get("Collider")) {
            let p: [f32; 3] = serde_json::from_value(t["position"].clone()).unwrap();
            let h: [f32; 3] = serde_json::from_value(c["halfExtents"].clone()).unwrap();
            scene
                .add_box(
                    [p[0] - h[0], p[1] - h[1], p[2] - h[2]],
                    [p[0] + h[0], p[1] + h[1], p[2] + h[2]],
                    2,
                    id.as_str(),
                    MaterialPreset::Rock.material(),
                )
                .unwrap();
        }
    }
    scene.commit();
    let (x0, z0) = (x as f32 * TILE, z as f32 * TILE);
    let positions = floor_probes(
        &scene,
        [x0 + 4.0, -20.0, z0 + 4.0],
        [x0 + TILE - 4.0, 40.0, z0 + TILE - 4.0],
        8.0,
        1.6,
    );
    let mut baker = GeometricBaker {
        rays: 256,
        bounces: 8,
        ..Default::default()
    };
    AcousticCell {
        key: CellKey { x, z, slot: 0 },
        baker: baker.name(),
        probes: baker.bake(&scene, &positions).unwrap(),
    }
    .encode()
    .unwrap()
}

/// A mono PCM16 WAV at 24 kHz (the importer resamples to the engine rate).
fn wav(samples: &[f32]) -> Vec<u8> {
    let rate = 24_000u32;
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes())
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// Seeded xorshift noise in [-1, 1].
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// Fire crackle (1 s loop), a calm pad and a drum stem (one 120 BPM bar each).
fn sounds() -> Vec<(&'static str, &'static str, Vec<u8>)> {
    let rate = 24_000.0;
    let tau = std::f32::consts::TAU;
    let mut noise = Noise(0x2545_f491);
    let crackle: Vec<f32> = (0..24_000)
        .map(|i| {
            let hiss = noise.next() * 0.04;
            let pop = if noise.next() > 0.995 {
                noise.next() * 0.7
            } else {
                0.0
            };
            let rumble = (i as f32 / rate * 70.0 * tau).sin() * 0.05;
            hiss + pop + rumble
        })
        .collect();
    let pad: Vec<f32> = (0..48_000)
        .map(|i| {
            let t = i as f32 / rate;
            let chord = [220.0, 277.18, 329.63, 440.0]
                .iter()
                .map(|f| (t * f * tau).sin())
                .sum::<f32>();
            chord * 0.08 * (0.8 + 0.2 * (t * 0.5 * tau).sin())
        })
        .collect();
    let mut noise = Noise(0x1234_5678);
    let drums: Vec<f32> = (0..48_000)
        .map(|i| {
            let t = i as f32 / rate;
            let beat = t % 0.5;
            let kick =
                (beat * (60.0 + 90.0 * (-beat * 30.0).exp()) * tau).sin() * (-beat * 9.0).exp();
            let off = (t + 0.25) % 0.5;
            let hat = noise.next() * (-off * 60.0).exp() * 0.25;
            (kick * 0.6 + hat) * 0.8
        })
        .collect();
    vec![
        ("audio/crackle.wav", "as_crackle", wav(&crackle)),
        ("audio/calm.wav", "as_calm", wav(&pad)),
        ("audio/drums.wav", "as_drums", wav(&drums)),
    ]
}

/// A small faceted rock (icosahedron), as a glTF binary with flat normals.
fn rock_glb() -> Vec<u8> {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let raw = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ];
    let faces: [[usize; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    // Squashed and jittered so it reads as a rock, unit-sized.
    let points: Vec<glam::Vec3> = raw
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let jitter = 1.0 + ((i * 7919) % 13) as f32 / 40.0;
            glam::Vec3::from_array(*p).normalize() * 0.5 * jitter * glam::Vec3::new(1.0, 0.75, 1.0)
        })
        .collect();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    for f in faces {
        let [a, b, c] = f.map(|i| points[i]);
        let n = (b - a).cross(c - a).normalize();
        for p in [a, b, c] {
            positions.push(p);
            normals.push(n);
        }
    }
    let count = positions.len();
    let mut bin = Vec::new();
    for p in &positions {
        for v in p.to_array() {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    for n in &normals {
        for v in n.to_array() {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    let (min, max) = positions.iter().fold(
        (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
    );
    let gltf = json!({
        "asset": {"version": "2.0", "generator": "ara valley generator"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "Rock"}],
        "meshes": [{"name": "Rock", "primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1}, "material": 0}]}],
        "materials": [{"name": "Stone", "pbrMetallicRoughness": {"baseColorFactor": [0.42, 0.4, 0.37, 1], "metallicFactor": 0, "roughnessFactor": 0.92}}],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": count * 12},
            {"buffer": 0, "byteOffset": count * 12, "byteLength": count * 12}
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": count, "type": "VEC3",
                "min": min.to_array(), "max": max.to_array()},
            {"bufferView": 1, "componentType": 5126, "count": count, "type": "VEC3"}
        ]
    });
    let mut text = serde_json::to_vec(&gltf).unwrap();
    while text.len() % 4 != 0 {
        text.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let mut glb = Vec::new();
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&((12 + 8 + text.len() + 8 + bin.len()) as u32).to_le_bytes());
    glb.extend_from_slice(&(text.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&text);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&bin);
    glb
}

/// An import sidecar authored through its typed command and canonical writer.
fn sidecar(id: &str, importer: &str, version: &str, settings: Value) -> String {
    use ara_asset::{ImportSettings, Sidecar, SidecarCommand};
    let mut schema = SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_asset::register_schema(&mut schema);
    let id = AssetId::parse(id).unwrap();
    let mut workspace = Workspace::default();
    workspace.insert("sidecar", Sidecar::empty(id.clone()).into_document());
    let mut store = ara_ops::Store::new(schema, workspace);
    let edit = SidecarCommand::Initialize(ImportSettings {
        importer: importer.into(),
        version: version.into(),
        settings,
        ..Default::default()
    })
    .edit(id.clone())
    .unwrap();
    store
        .transact(
            ara_ops::TxnMeta::by(ara_ops::Author::Agent("valley".into())),
            [edit],
        )
        .unwrap();
    ara_doc::write(store.workspace().doc(id.as_str()).unwrap(), store.schema())
}

/// The music: one 120 BPM bar looping, the pad always on, drums above intensity 0.5.
fn music() -> Document {
    document(
        "music",
        MUSIC.1,
        vec![
            (
                "ms_valley",
                serde_json::from_value(json!({"initial": "calm", "intensity": 0.7, "layerFade": 1, "seed": 7})).unwrap(),
            ),
            (
                "sg_calm",
                serde_json::from_value(json!({"name": "calm", "bars": 1,
                    "tempo": [{"bar": 0, "bpm": 120, "beatsPerBar": 4}],
                    "stems": [
                        {"name": "pad", "clip": "@audio/calm.wav#as_calm", "intensity": [0, 0], "gainDb": -6},
                        {"name": "drums", "clip": "@audio/drums.wav#as_drums", "intensity": [0.4, 0.6], "gainDb": -8}
                    ]}))
                .unwrap(),
            ),
        ],
    )
}

/// The world manifest: four 32 m cells, one art layer streamed within 6 m.
fn world() -> Document {
    let mut records = vec![
        ("w_grid", serde_json::from_value(json!({"cellSize": TILE})).unwrap()),
        (
            "l_art",
            serde_json::from_value(json!({"name": "Art", "loaded": true, "visible": true, "range": 6, "graceTicks": 120})).unwrap(),
        ),
    ];
    for (x, z) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let id: &'static str = Box::leak(format!("c_{x}{z}").into_boxed_str());
        records.push((
            id,
            serde_json::from_value(json!({"x": x, "z": z, "slot": 0, "layer": "l_art",
                "documents": [format!("@world/cells/c{x}{z}.scene#sc_cell{x}{z}")],
                "acoustics": format!("@world/cells/c{x}{z}.acoustics#as_ac{x}{z}")}))
            .unwrap(),
        ));
    }
    document("world", WORLD.1, records)
}

/// The level: environment, sun, the terrain (streamed world, navigation, the camp's
/// chunk), the Fox, its follow camera, the guard, the campfire, music, camp props.
fn level(data: &TerrainData) -> Document {
    let h = |x: f32, z: f32| ground(data, x, z);
    let mut records: Vec<(&str, Record)> = vec![
        (
            "e_world",
            entity(
                "World",
                json!({"Environment": {"sky": [0.16, 0.3, 0.62, 1], "horizon": [0.92, 0.62, 0.4, 1],
                    "ground": [0.08, 0.07, 0.05, 1], "intensity": 260, "exposure": 0.0022}}),
            ),
        ),
        (
            "e_sun",
            entity(
                "Sun",
                json!({"Transform": {"rotation": [-32, -40, 0]},
                    "Light": {"kind": "directional", "color": [1.0, 0.72, 0.5, 1], "intensity": 2600}}),
            ),
        ),
        (
            "e_terrain",
            entity(
                "Valley",
                json!({
                    "Terrain": {"terrain": reference(TERRAIN), "foliage": reference(FOLIAGE)},
                    "StreamedWorld": {"world": reference(WORLD)},
                    "TerrainChunk": {"x": 0, "z": 0},
                    "AcousticSurface": {"preset": "gravel"},
                    "NavMeshSurface": {"profiles": [0], "min": [0, -10, 0], "max": [64, 30, 64],
                        "cellSize": 0.25, "tileSize": 64},
                }),
            ),
        ),
        (
            "e_foxkind",
            entity(
                "Fox agents",
                json!({"NavAgentType": {"profile": 0, "name": "Fox", "radius": 0.3, "height": 1.0, "climb": 0.4, "slope": 40}}),
            ),
        ),
        (
            "e_fox",
            entity(
                "Fox",
                json!({
                    "Transform": {"position": [START[0], h(START[0], START[1]) + 0.05, START[1]], "rotation": [0, 90, 0], "scale": [0.01, 0.01, 0.01]},
                    "MeshRenderer": {"model": FOX, "roughness": 0.58},
                    "Animator": {"controller": "@anim/fox.animation#an_foxmatch"},
                    "CharacterController": {"radius": 0.3, "height": 0.9, "crouchHeight": 0.7, "stepHeight": 0.3, "stickDown": 0.3},
                    "PhysicsId": {"value": 20},
                    "PlayerControl": {"camera": "e_camera", "yaw": 90},
                    "Stimulus": {"kind": "player", "height": 0.4},
                }),
            ),
        ),
        (
            "e_camera",
            entity(
                "Camera",
                json!({"Transform": {"position": [START[0] - 5.0, 3, START[1]]}, "Camera": {"fov": 55},
                    "FollowCamera": {"target": "e_fox", "distance": 6.5, "height": 0.8, "pitch": 20, "yaw": 90}}),
            ),
        ),
        (
            "e_guard",
            entity(
                "Guard",
                json!({
                    "Transform": {"position": [26, h(26.0, 24.0), 24], "scale": [0.011, 0.011, 0.011]},
                    "MeshRenderer": {"model": FOX, "color": [0.55, 0.65, 1.0, 1], "roughness": 0.6},
                    "Animator": {"controller": "@anim/fox.animation#an_fox"},
                    "NavAgent": {"profile": 0, "radius": 0.3, "height": 0.9, "maxSpeed": 3.2},
                    "Brain": {"behavior": reference(GUARD), "overrides": [format!(
                        "route = [vec3(26, {:.2}, 24), vec3(27, {:.2}, 7), vec3(8, {:.2}, 9), vec3(9, {:.2}, 25)]",
                        h(26.0, 24.0), h(27.0, 7.0), h(8.0, 9.0), h(9.0, 25.0))]},
                    "Perceiver": {"sightRange": 12, "fov": 110, "eyeHeight": 0.6},
                    "RigidBody": {"motion": "kinematic"},
                    "Collider": {"kind": "capsule", "radius": 0.3, "halfHeight": 0.2},
                    "PhysicsId": {"value": 30},
                }),
            ),
        ),
        (
            "e_fire",
            entity(
                "Campfire",
                json!({
                    "Transform": {"position": [16, h(16.0, 16.0), 16]},
                    "VisualEffect": {"effect": "campfire"},
                    "AudioSource": {"clip": "@audio/crackle.wav#as_crackle", "looping": true, "referenceDistance": 2, "maxDistance": 40},
                    "AcousticSource": {},
                }),
            ),
        ),
        (
            "e_music",
            entity(
                "Music",
                json!({"MusicSource": {"music": reference(MUSIC), "intensity": 0.7}}),
            ),
        ),
    ];
    for (i, (x, z)) in [(13.0f32, 15.0f32), (19.0, 18.5)].iter().enumerate() {
        let rid: &'static str = Box::leak(format!("e_log{i}").into_boxed_str());
        records.push((
            rid,
            entity(
                "Log bench",
                json!({"Transform": {"position": [x, h(*x, *z) + 0.2, z], "rotation": [0, 30.0 + 60.0 * i as f32, 0]},
                    "MeshRenderer": {"size": [1.8, 0.4, 0.45], "color": [0.35, 0.22, 0.12, 1], "roughness": 0.85},
                    "RigidBody": {}, "Collider": {"halfExtents": [0.9, 0.2, 0.22]},
                    "PhysicsId": {"value": 40 + i as u64}, "AcousticSurface": {"preset": "wood"}}),
            ),
        ));
    }
    for (i, (x, z, color)) in [
        (18.5f32, 13.0f32, [0.8, 0.35, 0.12]),
        (18.6, 13.2, [0.85, 0.55, 0.15]),
        (17.8, 12.4, [0.2, 0.45, 0.8]),
    ]
    .iter()
    .enumerate()
    {
        let rid: &'static str = Box::leak(format!("e_crate{i}").into_boxed_str());
        let mut record = crate_box(data, *x, *z, 0.6, *color, 50 + i as u64);
        if i == 1 {
            // Stacked on the first.
            record["Transform"]["position"][1] = json!(h(*x, *z) + 0.92);
        }
        records.push((rid, record));
    }
    for (i, record) in lamp(data, 12.5, 19.5, [1.0, 0.55, 0.25])
        .into_iter()
        .chain(lamp(data, 21.0, 11.5, [1.0, 0.6, 0.3]))
        .enumerate()
    {
        let rid: &'static str = Box::leak(format!("e_lamp{i}").into_boxed_str());
        records.push((rid, record));
    }
    document("scene", SCENE.1, records)
}

/// Every Valley file (project path → bytes), authored through typed commands and
/// canonical writers. Deterministic: the same code writes the same bytes.
fn valley_documents() -> Vec<(String, Vec<u8>)> {
    let (db, registry) = engine();
    let mut workspace = Workspace::default();
    workspace.insert(
        TERRAIN.0,
        Document::new("terrain", AssetId::parse(TERRAIN.1).unwrap()),
    );
    workspace.insert(
        FOLIAGE.0,
        Document::new("foliage", AssetId::parse(FOLIAGE.1).unwrap()),
    );
    // Seeded IDs: generated stroke and rule records are the same every run.
    let mut store = ara_ops::Store::with_ids(db.clone(), workspace, ara_doc::IdGen::seeded(7));
    terrain(&mut store, &registry);
    foliage(&mut store, &registry);
    let data = evaluate(store.workspace().doc(TERRAIN.1).unwrap());
    let text = |doc: &Document| ara_doc::write(doc, &db).into_bytes();
    let mut files = vec![
        (
            TERRAIN.0.to_owned(),
            text(store.workspace().doc(TERRAIN.1).unwrap()),
        ),
        (
            FOLIAGE.0.to_owned(),
            text(store.workspace().doc(FOLIAGE.1).unwrap()),
        ),
        (GUARD.0.to_owned(), text(&guard_graph())),
        (MUSIC.0.to_owned(), text(&music())),
        (WORLD.0.to_owned(), text(&world())),
        (SCENE.0.to_owned(), text(&level(&data))),
    ];
    for (x, z) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let doc = cell(&data, x, z);
        let path = format!("world/cells/c{x}{z}");
        files.push((format!("{path}.acoustics"), acoustics(&data, &doc, x, z)));
        files.push((
            format!("{path}.acoustics.import.json"),
            sidecar(&format!("as_ac{x}{z}"), "ara.acoustics", "1", Value::Null).into_bytes(),
        ));
        files.push((format!("{path}.scene"), text(&doc)));
    }
    for (path, id, bytes) in sounds() {
        files.push((path.to_owned(), bytes));
        files.push((
            format!("{path}.import.json"),
            sidecar(
                id,
                "ara.audio",
                "2",
                json!({"usage": "shortEffect", "codec": "pcm"}),
            )
            .into_bytes(),
        ));
    }
    let gltf = ara_importers::registry().unwrap();
    let gltf_version = gltf.get("ara.gltf").unwrap().version().to_owned();
    files.push(("assets/props/rock.glb".into(), rock_glb()));
    files.push((
        "assets/props/rock.glb.import.json".into(),
        sidecar("as_rock", "ara.gltf", &gltf_version, Value::Null).into_bytes(),
    ));
    files
}

#[test]
fn valley_documents_are_canonical_and_current() {
    let (db, _) = engine();
    for (relative, expected) in valley_documents() {
        let path = demo().join(&relative);
        if std::env::var_os("ARA_UPDATE_DEMO").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &expected).unwrap();
        }
        assert!(
            std::fs::read(&path).unwrap_or_default() == expected,
            "{relative}: regenerate with ARA_UPDATE_DEMO=1"
        );
        // Text documents read back strictly in the engine schema world.
        if !relative.ends_with(".import.json")
            && let Ok(text) = std::str::from_utf8(&expected)
            && text.starts_with('{')
        {
            let read = ara_doc::read(text, &relative, &db, None);
            assert!(
                read.diags.iter().all(|d| !d.is_error()),
                "{relative}: {:?}",
                read.diags
            );
        }
    }
}

/// The Fox and the rock, cooked once in-process (players load the same artifacts).
fn cooked() -> &'static BTreeMap<String, CachedImport> {
    static COOKED: OnceLock<BTreeMap<String, CachedImport>> = OnceLock::new();
    COOKED.get_or_init(|| {
        let registry = ara_importers::registry().unwrap();
        let cook = |path: &str| {
            let input = ImportInput {
                preset: ara_asset::CookPreset::Fast,
                thread_budget: 4,
                source_path: path.into(),
                source: std::fs::read(demo().join(path)).unwrap(),
                importer: "ara.gltf".into(),
                version: registry.get("ara.gltf").unwrap().version().into(),
                settings: Value::Null,
                target: TargetPlatform::Windows,
                dependencies: Default::default(),
                asset_dependencies: Default::default(),
            };
            let cancellation = Cancellation::default();
            let mut progress = |_| {};
            let mut context = ImportContext {
                metrics: None,
                cancellation: &cancellation,
                progress: &mut progress,
            };
            let output = registry.import(&input, &mut context).unwrap();
            CachedImport {
                key: input.key().unwrap(),
                output,
            }
        };
        BTreeMap::from([
            ("as_fox".to_owned(), cook("assets/fox/Fox.glb")),
            ("as_rock".to_owned(), cook("assets/props/rock.glb")),
        ])
    })
}

/// Imported assets of the demo project (sidecar `id` → source path), as hosts index
/// them and play snapshots carry them.
fn imports(dir: &Path, index: &mut ara_doc::AssetIndex) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            imports(&path, index);
        } else if let Some(source) = path.to_str().and_then(|p| p.strip_suffix(".import.json")) {
            let sidecar: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let relative = Path::new(source)
                .strip_prefix(demo())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            index.insert(sidecar["id"].as_str().unwrap(), relative, None);
        }
    }
}

/// Every text document the Valley plays with, read strictly from the demo project.
fn workspace(db: &SchemaDb) -> Workspace {
    let mut index = ara_doc::AssetIndex::new();
    imports(&demo(), &mut index);
    let mut workspace = Workspace::new(index);
    let mut paths = vec![
        SCENE.0.to_owned(),
        TERRAIN.0.to_owned(),
        FOLIAGE.0.to_owned(),
        WORLD.0.to_owned(),
        GUARD.0.to_owned(),
        MUSIC.0.to_owned(),
        "anim/fox.animation".to_owned(),
    ];
    paths.extend(
        [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(x, z)| format!("world/cells/c{x}{z}.scene")),
    );
    for path in paths {
        let text = std::fs::read_to_string(demo().join(&path)).unwrap();
        let read = ara_doc::read(&text, &path, db, None);
        assert!(
            read.diags.iter().all(|d| !d.is_error()),
            "{path}: {:?}",
            read.diags
        );
        workspace.insert(&path, read.doc.unwrap());
    }
    workspace
}

/// The Valley in-process with settled streaming (reproducible arrivals).
fn valley(seed: u64) -> Runtime {
    let db = ara_runtime::schema(&[]).unwrap();
    let workspace = workspace(&db);
    let scene = AssetId::parse(SCENE.1).unwrap();
    let (streaming, _) = WorldStreaming::from_workspace(&workspace, scene.as_str())
        .unwrap()
        .expect("the valley streams its world");
    Runtime::new(
        db,
        workspace,
        scene,
        Box::new(MemoryAssets(cooked().clone())),
        RuntimeConfig {
            seed,
            ..Default::default()
        },
        vec![Box::new(streaming.settled(Duration::from_secs(20)))],
    )
    .unwrap()
}

/// Runs east: the follow camera faces +X, so forward on `Move` with `Sprint` held.
fn walk_east(ticks: u32) -> Vec<StepInput> {
    let held = |action: &str, value| StepInput {
        after_ticks: 0,
        input: Injection {
            player: PlayerId(1),
            action: action.into(),
            value,
            hold_ticks: ticks,
        },
    };
    vec![
        held("Move", ActionValue::Axis2([0.0, 1.0])),
        held("Sprint", ActionValue::Button(true)),
    ]
}

fn component(rt: &Runtime, entity: &str, name: &str) -> Value {
    rt.play()
        .session()
        .component(entity, name)
        .unwrap()
        .unwrap_or(Value::Null)
}

fn position(rt: &Runtime, entity: &str) -> [f32; 3] {
    serde_json::from_value(component(rt, entity, "Transform")["position"].clone()).unwrap()
}

fn resident(rt: &Runtime, entity: &str) -> bool {
    rt.play().session().entity(entity).is_some()
}

fn guard_state(rt: &Runtime) -> (ara_ai::BrainState, ara_ai::LocomotionIntent) {
    let world = rt.world();
    let entity = rt.play().session().entity("e_guard").unwrap();
    (
        world.get::<ara_ai::BrainState>(entity).unwrap().clone(),
        *world.get::<ara_ai::LocomotionIntent>(entity).unwrap(),
    )
}

#[test]
fn valley_streams_cells_grounds_the_fox_and_the_guard_hunts_on_streamed_navigation() {
    let mut rt = valley(3);
    // Tick 1: the camp cell arrives (settled streaming), the far cells do not.
    rt.step(1, &[]).unwrap();
    assert!(resident(&rt, "e_c00rock0"));
    assert!(!resident(&rt, "e_c10ground") && !resident(&rt, "e_c11tower"));
    let nav = rt
        .world()
        .get_resource::<ara_runtime::navigation::NavStreaming>()
        .unwrap();
    assert_eq!(
        nav.resident()[&0].len(),
        1,
        "camp nav cell published at load"
    );
    // Walk east across the cell border; cell (1, 0) streams in ahead of the Fox.
    let start = position(&rt, "e_fox");
    let report = rt.step(360, &walk_east(360)).unwrap();
    assert_eq!(report.ticks, 360);
    assert!(rt.fault().is_none(), "{:?}", rt.fault());
    let end = position(&rt, "e_fox");
    assert!(
        end[0] > 33.0,
        "the Fox crossed into cell (1, 0): {start:?} -> {end:?}"
    );
    assert!(resident(&rt, "e_c10ground") && resident(&rt, "e_c10crate0"));
    let motion = component(&rt, "e_fox", "CharacterMotion");
    assert_eq!(motion["grounded"], json!(true), "{motion}");
    let terrain = ara_runtime::terrain::TerrainState::clone(rt.world().resource());
    let floor = terrain.0.height(end[0], end[2]).unwrap();
    assert!(
        (end[1] - floor).abs() < 0.3,
        "on the terrain: {end:?} vs {floor}"
    );
    // Navigation followed the resident chunks: a swap published both cells.
    let nav = rt
        .world()
        .get_resource::<ara_runtime::navigation::NavStreaming>()
        .unwrap();
    let swap = nav.swaps.last().unwrap();
    assert!(swap.tick > 0 && swap.cells.len() >= 2, "{:?}", nav.swaps);
    // The guard saw the Fox and moved; animation follows its locomotion intent.
    let (brain, locomotion) = guard_state(&rt);
    let guard = position(&rt, "e_guard");
    assert!(brain.age >= 361, "{brain:?}");
    assert!(
        (guard[0] - 26.0).abs() + (guard[2] - 24.0).abs() > 3.0,
        "the guard moved: {guard:?} ({:?})",
        brain.last
    );
    let speed = (locomotion.velocity[0].powi(2) + locomotion.velocity[2].powi(2)).sqrt();
    println!(
        "VALLEY fox {start:?} -> {end:?}, guard at {guard:?} speed {speed:.2} state {:?}, nav swaps {:?}",
        brain.last, nav.swaps
    );
}

#[test]
fn valley_is_deterministic_checkpoints_are_exact_and_replays_verify() {
    let mut a = valley(9);
    let mut b = valley(9);
    let inputs = walk_east(240);
    a.step(240, &inputs).unwrap();
    b.step(240, &inputs).unwrap();
    let hash = |rt: &Runtime| rt.play().session().state_hash().unwrap();
    assert_eq!(hash(&a), hash(&b), "same seed and inputs, same state");
    // Exact checkpoint across a stream arrival and nav swap.
    let checkpoint = a.play_mut().checkpoint().unwrap();
    a.play_mut().record().unwrap();
    a.step(120, &walk_east(120)).unwrap();
    let after = hash(&a);
    let replay = a.play_mut().finish_recording().unwrap();
    a.restore(&checkpoint).unwrap();
    a.step(120, &walk_east(120)).unwrap();
    assert_eq!(hash(&a), after, "restored checkpoint replays exactly");
    // The recording (with its streamed arrivals) verifies tick by tick.
    assert_eq!(a.replay(&replay).unwrap(), after);
}
