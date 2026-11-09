//! The BLAS input (an indexed triangle mesh split into submeshes) and its content hash.
use sha2::{Digest, Sha256};
use std::fmt;

/// One mesh asset as `trace()` sees it: positions, triangle indices and the first
/// triangle of each submesh (material slot). Triangle `i` is indices `3i..3i+3`; its
/// stable primitive id is `i` and its submesh the last start `<= i`.
#[derive(Debug, Clone, Copy)]
pub struct Mesh<'a> {
    /// Object-space vertex positions in metres.
    pub positions: &'a [[f32; 3]],
    /// Three indices per triangle, counter-clockwise for the front face.
    pub indices: &'a [u32],
    /// First triangle of each submesh, ascending and starting at zero; empty means
    /// one submesh.
    pub submeshes: &'a [u32],
}

/// SHA-256 of a mesh's canonical bytes ([`Mesh::hash`]): the identity a BLAS and its
/// side-car are keyed by. Equal hashes mean equal positions, indices and submeshes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshHash(pub [u8; 32]);

impl MeshHash {
    /// Lower-case hex, used in side-car file names.
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for MeshHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshHash({})", &self.hex()[..16])
    }
}

/// Why a mesh cannot build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// The index count is not a multiple of three.
    PartialTriangle(usize),
    /// An index points past the positions.
    IndexOutOfRange {
        /// Triangle holding the index.
        triangle: u32,
        /// The offending index.
        index: u32,
    },
    /// Submesh starts must ascend from zero and stay below the triangle count.
    Submeshes,
    /// More triangle references than 32-bit addresses allow.
    TooLarge(usize),
    /// Side-car bytes are truncated, corrupt or for another mesh or builder.
    Sidecar(&'static str),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartialTriangle(n) => write!(f, "{n} indices do not form whole triangles"),
            Self::IndexOutOfRange { triangle, index } => {
                write!(
                    f,
                    "triangle {triangle} uses index {index} past the positions"
                )
            }
            Self::Submeshes => write!(f, "submesh starts must ascend from zero"),
            Self::TooLarge(n) => write!(f, "{n} triangles exceed the 32-bit BLAS limits"),
            Self::Sidecar(why) => write!(f, "BVH side-car rejected: {why}"),
        }
    }
}

impl std::error::Error for BuildError {}

impl Mesh<'_> {
    /// Number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The content hash: SHA-256 over a tag, then the positions' f32 bits, indices
    /// and submesh starts, each array prefixed by its length (all little-endian).
    pub fn hash(&self) -> MeshHash {
        let mut h = Sha256::new();
        h.update(b"ara-bvh-mesh-1");
        h.update((self.positions.len() as u64).to_le_bytes());
        for p in self.positions {
            for v in p {
                h.update(v.to_bits().to_le_bytes());
            }
        }
        h.update((self.indices.len() as u64).to_le_bytes());
        for i in self.indices {
            h.update(i.to_le_bytes());
        }
        h.update((self.submeshes.len() as u64).to_le_bytes());
        for s in self.submeshes {
            h.update(s.to_le_bytes());
        }
        MeshHash(h.finalize().into())
    }

    /// Checks the indices and submesh table.
    pub fn validate(&self) -> Result<(), BuildError> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(BuildError::PartialTriangle(self.indices.len()));
        }
        let triangles = self.triangle_count();
        // References may grow by half through spatial splits; four 16-byte words each.
        if triangles > (1 << 28) {
            return Err(BuildError::TooLarge(triangles));
        }
        for (t, tri) in self.indices.as_chunks::<3>().0.iter().enumerate() {
            if let Some(&index) = tri.iter().find(|&&i| i as usize >= self.positions.len()) {
                return Err(BuildError::IndexOutOfRange {
                    triangle: t as u32,
                    index,
                });
            }
        }
        let starts = self.submeshes;
        if !starts.is_empty()
            && (starts[0] != 0
                || starts.windows(2).any(|w| w[0] >= w[1])
                || (triangles > 0 && *starts.last().unwrap_or(&0) as usize >= triangles))
        {
            return Err(BuildError::Submeshes);
        }
        Ok(())
    }

    /// Submesh of triangle `t`.
    pub fn submesh_of(&self, t: u32) -> u32 {
        self.submeshes
            .partition_point(|&s| s <= t)
            .saturating_sub(1) as u32
    }

    /// Corners of triangle `t`.
    pub fn corners(&self, t: u32) -> [glam::Vec3; 3] {
        let i = t as usize * 3;
        std::array::from_fn(|k| glam::Vec3::from(self.positions[self.indices[i + k] as usize]))
    }
}
