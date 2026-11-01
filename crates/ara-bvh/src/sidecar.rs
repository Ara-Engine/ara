//! Side-car bytes for one BLAS, cached next to a project's imported data and keyed by
//! the mesh content hash, so a mesh builds once (at import, or on first load for old
//! projects) without touching the shared asset format. The caller owns the I/O.
//!
//! Layout (little-endian): magic `ARABVH`, format and [`BUILDER_VERSION`] words,
//! node and triangle-reference counts, the [`BlasStats`] words, the bounds (6 f32),
//! the mesh hash, the node and triangle words, then SHA-256 of everything before it.
use crate::cwbvh::{Blas, BlasStats, NODE_WORDS, TRIANGLE_WORDS};
use crate::{Aabb, BUILDER_VERSION, BuildError, MeshHash, TRACE_STACK};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 6] = b"ARABVH";
const FORMAT: u16 = 1;
/// Magic, format, nine words, the bounds and the mesh hash.
const HEADER: usize = 6 + 2 + 9 * 4 + 6 * 4 + 32;

/// File name of `hash`'s side-car: the hash and builder version, so builder
/// changes never read stale trees.
pub fn file_name(hash: MeshHash) -> String {
    format!("{}-v{BUILDER_VERSION}.cwbvh", hash.hex())
}

/// Serializes `blas`.
pub fn encode(blas: &Blas) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + blas.bytes() + 32);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    let s = &blas.stats;
    let words = [
        BUILDER_VERSION,
        (blas.nodes.len() / NODE_WORDS) as u32,
        (blas.triangles.len() / TRIANGLE_WORDS) as u32,
        s.triangles,
        s.degenerate,
        s.references,
        s.nodes,
        s.max_stack,
        s.submeshes,
    ];
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    for v in blas
        .bounds
        .min
        .to_array()
        .into_iter()
        .chain(blas.bounds.max.to_array())
    {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&blas.hash.0);
    for w in blas.nodes.iter().chain(&blas.triangles) {
        out.extend_from_slice(bytemuck::cast_slice(w));
    }
    let digest: [u8; 32] = Sha256::digest(&out).into();
    out.extend_from_slice(&digest);
    out
}

/// Reads side-car `bytes`, accepting them only for mesh `expected`, this builder
/// version, an intact checksum and a structurally valid tree.
pub fn decode(bytes: &[u8], expected: MeshHash) -> Result<Blas, BuildError> {
    let fail = BuildError::Sidecar;
    if bytes.len() < HEADER + 32 || &bytes[..6] != MAGIC {
        return Err(fail("not a BVH side-car"));
    }
    let (body, digest) = bytes.split_at(bytes.len() - 32);
    if Sha256::digest(body).as_slice() != digest {
        return Err(fail("checksum mismatch"));
    }
    if u16::from_le_bytes([bytes[6], bytes[7]]) != FORMAT {
        return Err(fail("unknown format"));
    }
    let word = |i: usize| {
        let o = 8 + i * 4;
        u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
    };
    if word(0) != BUILDER_VERSION {
        return Err(fail("built by another builder version"));
    }
    let (nodes, refs) = (word(1) as usize, word(2) as usize);
    let stats = BlasStats {
        triangles: word(3),
        degenerate: word(4),
        references: word(5),
        nodes: word(6),
        max_stack: word(7),
        submeshes: word(8),
    };
    let float = |i: usize| f32::from_bits(word(9 + i));
    let bounds = Aabb {
        min: glam::Vec3::new(float(0), float(1), float(2)),
        max: glam::Vec3::new(float(3), float(4), float(5)),
    };
    let hash_at = 8 + 15 * 4;
    let hash = MeshHash(
        bytes[hash_at..hash_at + 32]
            .try_into()
            .map_err(|_| fail("hash"))?,
    );
    if hash != expected {
        return Err(fail("built from another mesh"));
    }
    let payload = &body[HEADER..];
    if payload.len() != (nodes * NODE_WORDS + refs * TRIANGLE_WORDS) * 16 {
        return Err(fail("truncated payload"));
    }
    let words: Vec<[u32; 4]> = payload
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| {
            std::array::from_fn(|k| {
                u32::from_le_bytes([c[k * 4], c[k * 4 + 1], c[k * 4 + 2], c[k * 4 + 3]])
            })
        })
        .collect();
    let (node_words, tri_words) = words.split_at(nodes * NODE_WORDS);
    let blas = Blas {
        hash,
        nodes: node_words.to_vec(),
        triangles: tri_words.to_vec(),
        bounds,
        stats,
    };
    if stats.nodes as usize != nodes || stats.references as usize != refs {
        return Err(fail("statistics disagree with the payload"));
    }
    if stats.max_stack > TRACE_STACK || !structure_ok(&blas) {
        return Err(fail("tree structure out of range"));
    }
    Ok(blas)
}

/// Every child and triangle offset a traversal can follow stays inside the BLAS.
fn structure_ok(blas: &Blas) -> bool {
    let nodes = (blas.nodes.len() / NODE_WORDS) as u32;
    let tris = blas.triangles.len() as u32;
    (0..nodes as usize).all(|n| {
        let w = &blas.nodes[n * NODE_WORDS..(n + 1) * NODE_WORDS];
        let imask = w[0][3] >> 24;
        let interior = imask.count_ones();
        let meta: Vec<u8> = w[1][2]
            .to_le_bytes()
            .into_iter()
            .chain(w[1][3].to_le_bytes())
            .collect();
        // One past the last triangle any leaf slot reaches.
        let leaf_end = meta
            .iter()
            .enumerate()
            .filter(|(slot, _)| imask & (1 << slot) == 0)
            .map(|(_, &m)| u32::from(m & 31) + u32::from(m >> 5).count_ones())
            .max()
            .unwrap_or(0);
        (interior == 0 || w[1][0] + interior <= nodes)
            && w[1][1] + leaf_end * TRIANGLE_WORDS as u32 <= tris
    })
}
