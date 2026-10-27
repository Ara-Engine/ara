//! Checks the built scene at the document/prefab boundary, without imported art.
use anyhow::{Context, Result, ensure};
use ara_doc::prefab::Resolved;
use ara_runtime::math::Pose;
use glam34::{Mat4, Vec3};
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

struct NoBases;

impl ara_terrain::cache::TileSource for NoBases {
    fn load(&self, hash: &str) -> std::result::Result<ara_terrain::Tile, ara_diag::Diagnostic> {
        Err(ara_terrain::invalid(format!(
            "unexpected terrain base {hash}"
        )))
    }
}

fn world_matrix(
    address: &str,
    rows: &Resolved,
    matrices: &mut BTreeMap<String, Mat4>,
) -> Result<Mat4> {
    if let Some(matrix) = matrices.get(address) {
        return Ok(*matrix);
    }
    let row = rows.get(address).context("missing parent entity")?;
    let transform = row
        .record
        .get("Transform")
        .map(|v| serde_json::from_value::<ara_components::Transform>(v.clone()))
        .transpose()?
        .unwrap_or_default();
    let local = Pose::from_transform(&transform).matrix();
    let matrix = match &row.parent {
        Some(parent) => world_matrix(parent, rows, matrices)? * local,
        None => local,
    };
    matrices.insert(address.to_owned(), matrix);
    Ok(matrix)
}

#[test]
fn authored_chunks_cover_the_outpost_world_grid() -> Result<()> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = workspace_root.join("samples/outpost");
    let project = ara_play_host::open_project(&root).map_err(|d| anyhow::anyhow!("{d}"))?;
    let (workspace, diagnostics) = project.load(&[]);
    ensure!(
        !project
            .diags
            .iter()
            .chain(diagnostics.values().flatten())
            .any(|d| d.is_error()),
        "sample documents must load: {:?}; {diagnostics:?}",
        project.diags
    );
    let id = workspace.find(super::SCENE.0).context("Outpost scene")?;
    let mut problems = Vec::new();
    let rows = workspace.resolve(id, &project.db, &mut problems);
    ensure!(problems.is_empty(), "prefab resolution: {problems:?}");
    let mut matrices = BTreeMap::new();
    for address in rows.keys() {
        world_matrix(address, &rows, &mut matrices)?;
    }
    let terrain = ara_terrain::Terrain::from_document(
        workspace
            .doc(super::TERRAIN.1)
            .context("terrain document")?,
    )
    .map_err(|d| anyhow::anyhow!("{d}"))?;
    let header = &terrain.header;
    let tiles: std::collections::BTreeSet<_> = rows
        .values()
        .filter_map(|row| row.record.get("TerrainChunk"))
        .map(|value| serde_json::from_value::<ara_runtime::terrain::TerrainChunk>(value.clone()))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|chunk| (chunk.x, chunk.z))
        .collect();
    let expected: std::collections::BTreeSet<_> = (header.min.z..header.max.z)
        .flat_map(|z| (header.min.x..header.max.x).map(move |x| (x, z)))
        .collect();
    let positions: BTreeMap<_, _> = matrices
        .iter()
        .map(|(id, matrix)| (id, matrix.transform_point3(Vec3::ZERO).to_array()))
        .collect();
    let mut geometry = BTreeMap::new();
    if std::env::var_os("ARA_AUDIT_GEOMETRY").is_some() {
        for (address, row) in &rows {
            let Some(renderer) = row.record.get("MeshRenderer") else {
                continue;
            };
            let Some(model) = renderer["model"].as_str() else {
                continue;
            };
            let path = model
                .trim_start_matches('@')
                .split('#')
                .next()
                .context("model path")?;
            let (lo, hi) = super::super::bounds::file(
                &crate::authored_path(&root, path),
                renderer["node"].as_str(),
            )?;
            let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for corner in 0..8 {
                let point = Vec3::new(
                    if corner & 1 == 0 { lo.x } else { hi.x },
                    if corner & 2 == 0 { lo.y } else { hi.y },
                    if corner & 4 == 0 { lo.z } else { hi.z },
                );
                let point = matrices[address].transform_point3(point);
                min = min.min(point);
                max = max.max(point);
            }
            geometry.insert(address, [min.to_array(), max.to_array()]);
        }
    }
    let look: serde_json::Value = serde_json::from_slice(&std::fs::read(crate::authored_path(
        &root,
        "look/outpost.look.json",
    ))?)?;
    let mut shots = look["shots"].clone();
    let mut ground = ara_runtime::terrain::TerrainData {
        header: header.clone(),
        tiles: BTreeMap::new(),
        foliage: Vec::new(),
        foliage_rules: Vec::new(),
    };
    if look["groundRelative"] == true {
        for shot in shots.as_array_mut().context("look shots")? {
            for field in ["position", "target"] {
                let point: [f32; 3] = serde_json::from_value(shot[field].clone())?;
                let coord = ara_terrain::TileCoord {
                    x: ((point[0] - header.origin[0]) / header.tile_width()).floor() as i32,
                    z: ((point[2] - header.origin[1]) / header.tile_width()).floor() as i32,
                };
                if let std::collections::btree_map::Entry::Vacant(entry) = ground.tiles.entry(coord)
                {
                    entry.insert(
                        terrain
                            .evaluate(coord, &NoBases)
                            .map_err(|d| anyhow::anyhow!("{d}"))?,
                    );
                }
                let height = ground
                    .height(point[0], point[2])
                    .context("shot lies outside terrain")?;
                shot[field] = json!([point[0], point[1] + height, point[2]]);
            }
        }
    }
    let output = workspace_root.join("target/ara-samples/outpost-layout.json");
    std::fs::create_dir_all(output.parent().context("audit output directory")?)?;
    std::fs::write(
        output,
        ara_json::to_canonical_string(&ara_json::to_value(&json!({
            "worldPositions": positions,
            "geometryBounds": geometry,
            "worldLookShots": shots,
            "residentTiles": tiles,
            "grid": header,
            "navigation": rows["e_navsurface"].record.get("NavMeshSurface"),
        }))?),
    )?;
    ensure!(
        tiles == expected,
        "Outpost renders its whole grid: {} resident tiles, {} required; road and mountain rim must remain on visible terrain",
        tiles.len(),
        expected.len()
    );
    Ok(())
}
