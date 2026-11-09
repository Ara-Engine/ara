//! Versioned binary side-cars. Filesystem policy belongs to the import/load service.

use crate::{Charts, Error, Fragment, Mesh, Patch, Settings, VERSION};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"ARAPCH01";
const PATCH_BYTES: usize = 76;
const FRAGMENT_BYTES: usize = 28;
const HEADER_BYTES: usize = 104;

/// Content-addressed mesh and metric variant. No instance or room identity enters it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// SHA-256 of source positions, triangle order, material slots and diffuse RGB.
    pub mesh: [u8; 32],
    /// SHA-256 of mesh hash, algorithm version and all chart settings.
    pub variant: [u8; 32],
}

impl Key {
    /// Compute a canonical little-endian content key without relying on struct padding.
    pub fn new(mesh: Mesh<'_>, settings: Settings) -> Self {
        let mut hash = Sha256::new();
        hash.update((mesh.positions.len() as u64).to_le_bytes());
        hash.update((mesh.triangles.len() as u64).to_le_bytes());
        for position in mesh.positions {
            for value in position {
                hash.update(value.to_le_bytes());
            }
        }
        for triangle in mesh.triangles {
            for index in triangle.vertices {
                hash.update(index.to_le_bytes());
            }
            hash.update(triangle.material.to_le_bytes());
            for value in triangle.albedo {
                hash.update(value.to_le_bytes());
            }
        }
        let mesh = hash.finalize().into();
        let mut hash = Sha256::new();
        hash.update(mesh);
        hash.update(VERSION.to_le_bytes());
        let mut bytes = Vec::new();
        write_settings(&mut bytes, settings);
        hash.update(bytes);
        Self {
            mesh,
            variant: hash.finalize().into(),
        }
    }

    /// Relative side-car path below a project-owned cache root such as `.ara/gpu/`.
    pub fn relative_path(self) -> String {
        let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        format!(
            "patches/v{VERSION}/{}/{}.bin",
            hex(self.mesh),
            hex(self.variant)
        )
    }
}

/// Result of an import/load request. `write_back` is absent on a valid cache hit.
pub struct Prepared {
    /// Immutable scaled charts for the caller.
    pub charts: Charts,
    /// Content key the supplied/read bytes must belong to.
    pub key: Key,
    /// New side-car bytes after a miss or corrupt/stale cache; service publishes them atomically.
    pub write_back: Option<Vec<u8>>,
}

/// Shared import-time and load-time fallback. Old projects need no asset-format migration.
/// Cache misses/corruption rebuild at the requested resolution; invalid mesh input is an error.
pub fn prepare(
    mesh: Mesh<'_>,
    settings: Settings,
    cached: Option<&[u8]>,
) -> Result<Prepared, Error> {
    crate::build::validate(mesh, settings)?;
    let key = Key::new(mesh, settings);
    if let Some(bytes) = cached
        && let Ok(charts) = decode(bytes, key, settings)
        && charts
            .fragments
            .iter()
            .all(|f| (f.triangle as usize) < mesh.triangles.len())
    {
        return Ok(Prepared {
            charts,
            key,
            write_back: None,
        });
    }
    let charts = crate::build(mesh, settings)?;
    let write_back = Some(encode(&charts, key));
    Ok(Prepared {
        charts,
        key,
        write_back,
    })
}

/// Encode padding-free, little-endian records with a SHA-256 integrity checksum.
pub fn encode(charts: &Charts, key: Key) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + 32
            + charts.patches.len() * PATCH_BYTES
            + charts.fragments.len() * FRAGMENT_BYTES,
    );
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&key.mesh);
    bytes.extend_from_slice(&key.variant);
    write_settings(&mut bytes, charts.settings);
    write_u32(&mut bytes, charts.patches.len() as u32);
    write_u32(&mut bytes, charts.fragments.len() as u32);
    for p in charts.patches.iter() {
        write_f32(&mut bytes, p.area);
        for value in p.centroid.into_iter().chain(p.normal).chain(p.albedo) {
            write_f32(&mut bytes, value);
        }
        write_u32(&mut bytes, p.material);
        for value in p.min.into_iter().chain(p.max) {
            write_f32(&mut bytes, value);
        }
        write_u32(&mut bytes, p.first_fragment);
        write_u32(&mut bytes, p.fragment_count);
    }
    for f in charts.fragments.iter() {
        write_u32(&mut bytes, f.triangle);
        for value in f.barycentric.into_iter().flatten() {
            write_f32(&mut bytes, value);
        }
    }
    let checksum = Sha256::digest(&bytes);
    bytes.extend_from_slice(&checksum);
    bytes
}

/// Decode only the requested version/content/metric; lengths are checked before allocation.
pub fn decode(bytes: &[u8], key: Key, settings: Settings) -> Result<Charts, Error> {
    if bytes.len() < HEADER_BYTES + 32 {
        return Err(Error::InvalidCache);
    }
    let (payload, checksum) = bytes.split_at(bytes.len() - 32);
    if Sha256::digest(payload).as_slice() != checksum {
        return Err(Error::InvalidCache);
    }
    let mut reader = Reader(payload);
    if reader.take::<8>()? != *MAGIC
        || reader.take::<32>()? != key.mesh
        || reader.take::<32>()? != key.variant
    {
        return Err(Error::InvalidCache);
    }
    let stored = Settings {
        edge: reader.f32()?,
        scale: reader.vec3()?,
        crease_cos: reader.f32()?,
        max_fragments: reader.u32()?,
    };
    if stored != settings {
        return Err(Error::InvalidCache);
    }
    let count = reader.u32()? as usize;
    let fragments = reader.u32()? as usize;
    if fragments > settings.max_fragments as usize
        || count > fragments
        || count.checked_mul(PATCH_BYTES).and_then(|n| {
            fragments
                .checked_mul(FRAGMENT_BYTES)
                .and_then(|f| n.checked_add(f))
        }) != Some(reader.0.len())
    {
        return Err(Error::InvalidCache);
    }
    let mut patches = Vec::with_capacity(count);
    let mut next_fragment = 0;
    for _ in 0..count {
        let patch = Patch {
            area: reader.f32()?,
            centroid: reader.vec3()?,
            normal: reader.vec3()?,
            albedo: reader.vec3()?,
            material: reader.u32()?,
            min: reader.vec3()?,
            max: reader.vec3()?,
            first_fragment: reader.u32()?,
            fragment_count: reader.u32()?,
        };
        if patch.area <= 0.0
            || patch.fragment_count == 0
            || patch.first_fragment != next_fragment
            || patch
                .first_fragment
                .checked_add(patch.fragment_count)
                .is_none_or(|n| n as usize > fragments)
            || patch.albedo.iter().any(|v| !(0.0..=1.0).contains(v))
            || (0..3).any(|i| {
                patch.min[i] > patch.max[i]
                    || patch.centroid[i] < patch.min[i]
                    || patch.centroid[i] > patch.max[i]
            })
            || (patch.normal.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() > 0.001
        {
            return Err(Error::InvalidCache);
        }
        next_fragment += patch.fragment_count;
        patches.push(patch);
    }
    if next_fragment as usize != fragments {
        return Err(Error::InvalidCache);
    }
    let mut coverage = Vec::with_capacity(fragments);
    for _ in 0..fragments {
        let triangle = reader.u32()?;
        let mut barycentric = [[0.0; 2]; 3];
        for bary in &mut barycentric {
            *bary = [reader.f32()?, reader.f32()?];
            if bary[0] < -1e-6 || bary[1] < -1e-6 || bary[0] + bary[1] > 1.000001 {
                return Err(Error::InvalidCache);
            }
        }
        coverage.push(Fragment {
            triangle,
            barycentric,
        });
    }
    Ok(Charts {
        settings,
        patches: patches.into_boxed_slice(),
        fragments: coverage.into_boxed_slice(),
    })
}

fn write_u32(bytes: &mut Vec<u8>, v: u32) {
    bytes.extend_from_slice(&v.to_le_bytes());
}
fn write_f32(bytes: &mut Vec<u8>, v: f32) {
    bytes.extend_from_slice(&v.to_le_bytes());
}
fn write_settings(bytes: &mut Vec<u8>, settings: Settings) {
    for value in [settings.edge]
        .into_iter()
        .chain(settings.scale)
        .chain([settings.crease_cos])
    {
        write_f32(bytes, value);
    }
    write_u32(bytes, settings.max_fragments);
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let (head, tail) = self.0.split_at_checked(N).ok_or(Error::InvalidCache)?;
        self.0 = tail;
        head.try_into().map_err(|_| Error::InvalidCache)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32, Error> {
        let value = f32::from_le_bytes(self.take()?);
        if value.is_finite() {
            Ok(value)
        } else {
            Err(Error::InvalidCache)
        }
    }
    fn vec3(&mut self) -> Result<[f32; 3], Error> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
}
