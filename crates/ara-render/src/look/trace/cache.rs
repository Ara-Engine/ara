//! On-load BLAS builds through a side-car cache keyed by mesh content hash
//! (`<dir>/<hash>-v<builder>.cwbvh`, `ara_bvh::sidecar`), so each mesh builds once
//! per project and old projects need no reimport. Hosts pass a project cache
//! directory (by convention `.ara/gpu/bvh/`, next to `.ara/gpu/probe-gi/`); tests use
//! one under `target/`.
use super::scene::TraceMesh;
use crate::instanced::Geometry;
use anyhow::{Context, Result};
use ara_bvh::{Blas, Mesh, sidecar};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where a BLAS came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// A valid side-car.
    Cache,
    /// Built now (and written to the cache when one is set).
    Built,
}

/// A directory of BLAS side-cars.
#[derive(Debug, Clone)]
pub(crate) struct BlasCache {
    dir: PathBuf,
}

impl BlasCache {
    /// A cache in `dir` (created on first write).
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The side-car path of `mesh`.
    pub(crate) fn path(&self, mesh: &Mesh<'_>) -> PathBuf {
        self.dir.join(sidecar::file_name(mesh.hash()))
    }

    /// Reads `mesh`'s side-car, or builds the BLAS and writes one. A stale, corrupt
    /// or foreign side-car is rebuilt and replaced; a failed write only costs the
    /// next load a rebuild.
    pub(crate) fn load_or_build(&self, mesh: &Mesh<'_>) -> Result<(Blas, Source)> {
        let path = self.path(mesh);
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(blas) = sidecar::decode(&bytes, mesh.hash())
        {
            return Ok((blas, Source::Cache));
        }
        let blas = Blas::build(mesh).context("building a trace BLAS")?;
        let _ = write_atomic(&path, &sidecar::encode(&blas));
        Ok((blas, Source::Built))
    }
}

/// Writes through a process-unique temporary file and a rename, so readers never
/// see a partial side-car.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The trace mesh of native geometry `g` (finest LOD, as the raster draws it), its
/// BLAS from `cache` when given, plus corner UVs when `alpha_uvs` (meshes that
/// alpha-tested materials use).
pub(crate) fn mesh_from_geometry(
    g: &Geometry,
    cache: Option<&BlasCache>,
    alpha_uvs: bool,
) -> Result<(TraceMesh, Source)> {
    let positions: Vec<[f32; 3]> = g.vertices.iter().map(|v| v.position).collect();
    let indices = g.lods.first().map_or(&[][..], |l| l.indices.as_slice());
    let mesh = Mesh {
        positions: &positions,
        indices,
        submeshes: &[],
    };
    let (blas, source) = match cache {
        Some(cache) => cache.load_or_build(&mesh)?,
        None => (Blas::build(&mesh)?, Source::Built),
    };
    let uvs = alpha_uvs.then(|| {
        Arc::new(
            indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| {
                    let v = t.map(|i| &g.vertices[i as usize]);
                    [
                        v[0].uv[0],
                        v[0].uv[1],
                        v[1].uv[0],
                        v[1].uv[1],
                        v[2].uv[0],
                        v[2].uv[1],
                        v[0].uv1[0],
                        v[0].uv1[1],
                        v[1].uv1[0],
                        v[1].uv1[1],
                        v[2].uv1[0],
                        v[2].uv1[1],
                    ]
                })
                .collect(),
        )
    });
    Ok((
        TraceMesh {
            blas: Arc::new(blas),
            uvs,
        },
        source,
    ))
}
