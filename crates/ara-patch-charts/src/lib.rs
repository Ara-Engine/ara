//! Deterministic 25 cm surface-patch charts for one mesh: the unit of Ara's indirect
//! light transport (council-v3 architecture D2). Pure data in, flat FP32 layouts out;
//! no GPU or asset I/O. Used by `ara-importers` and `ara-render` (`look::transport`).

#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod build;
pub mod cache;
mod clip;

pub use build::build;

/// Chart algorithm/side-car revision. Changes to chart ordering or arithmetic bump this.
pub const VERSION: u32 = 2;

/// Triangle order is the source primitive order, before any BVH permutation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    /// Indices into the source position array.
    pub vertices: [u32; 3],
    /// Material slot; fragments of different slots never share a patch.
    pub material: u32,
    /// Linear diffuse RGB, supplied by the material evaluator/importer.
    pub albedo: [f32; 3],
}

/// Borrowed, unquantized source mesh. Degenerate triangles have no surface coverage.
#[derive(Clone, Copy, Debug)]
pub struct Mesh<'a> {
    /// Mesh-local positions in metres.
    pub positions: &'a [[f32; 3]],
    /// Stable source triangle order, shared with the tracing BLAS input.
    pub triangles: &'a [Triangle],
}

/// Metric of the chart grid in scaled mesh space, before rigid placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Cell width in metres. Production transport uses 0.25.
    pub edge: f32,
    /// Signed instance scale. Nonuniform and mirrored scales are supported.
    pub scale: [f32; 3],
    /// Minimum adjacent face-normal dot product for merging (default: 45 degrees).
    pub crease_cos: f32,
    /// Explicit resource guard. Exceeding this returns an error, never coarser charts.
    pub max_fragments: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            edge: 0.25,
            scale: [1.0; 3],
            crease_cos: std::f32::consts::FRAC_1_SQRT_2,
            max_fragments: 16_000_000,
        }
    }
}

/// A covered triangle fragment, with barycentrics back to the original source triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fragment {
    /// Source primitive index (not the BLAS storage/permutation index).
    pub triangle: u32,
    /// Per corner, weights of source vertices 1 and 2; weight 0 is `1 - u - v`.
    pub barycentric: [[f32; 2]; 3],
}

/// A connected surface piece inside one 25 cm grid cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    /// Surface area in square metres, including the instance scale.
    pub area: f32,
    /// Area-weighted centroid in scaled mesh space.
    pub centroid: [f32; 3],
    /// Unit area-weighted geometric normal, following transformed triangle winding.
    pub normal: [f32; 3],
    /// Area-weighted linear diffuse RGB.
    pub albedo: [f32; 3],
    /// Original material slot.
    pub material: u32,
    /// Inclusive minimum of the clipped surface's bounds in scaled mesh space.
    pub min: [f32; 3],
    /// Inclusive maximum of the clipped surface's bounds in scaled mesh space.
    pub max: [f32; 3],
    /// First element in `Charts::fragments`.
    pub first_fragment: u32,
    /// Number of covered triangle fragments.
    pub fragment_count: u32,
}

/// Immutable deterministic charts, shared by instances with the same mesh and scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Charts {
    /// Settings used for this chart set.
    pub settings: Settings,
    /// Cell-lexicographic order, then material, normal hemisphere and first source fragment.
    pub patches: Box<[Patch]>,
    /// Patch-contiguous surface coverage.
    pub fragments: Box<[Fragment]>,
}

/// Invalid inputs and resource exhaustion are explicit; resolution is never lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid scale, grid spacing, crease threshold, index, albedo or position.
    InvalidInput,
    /// Coordinates exceed the signed 32-bit grid domain or FP32 output range.
    CoordinateRange,
    /// The caller's fragment limit or a 32-bit buffer index was exceeded.
    FragmentLimit,
    /// Side-car is truncated, corrupt, stale, or belongs to another mesh/settings key.
    InvalidCache,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "patch charts: {self:?}")
    }
}

impl std::error::Error for Error {}

impl Charts {
    /// Surface coverage for a patch, in stable source-triangle order.
    pub fn coverage(&self, patch: &Patch) -> &[Fragment] {
        let start = patch.first_fragment as usize;
        &self.fragments[start..start + patch.fragment_count as usize]
    }

    /// Resident array storage, including spare capacity (boxed arrays have none).
    pub fn resident_bytes(&self) -> usize {
        size_of_val(self.patches.as_ref()) + size_of_val(self.fragments.as_ref())
    }
}

#[cfg(test)]
mod tests;
