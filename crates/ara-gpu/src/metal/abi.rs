//! Metal Shader Converter binding ABI, limited to what Apple's converter guides
//! document (binding model, pipeline setup, reference). Anything the guides leave to
//! `metal_irconverter_runtime.h` (draw-parameter buffers, acceleration-structure
//! headers) waits for the installed header instead of being guessed here.
//!
//! Ara converts every stage with one explicit global root signature that mirrors the
//! Vulkan set-0 layout (`cooked::BINDING_LAYOUT`). Root constants come first, then
//! one 64-bit table address per descriptor table, so the top-level argument buffer
//! layout is identical for every pipeline.

/// `kIRArgumentBufferBindPoint`: argument-table buffer slot of the top-level buffer.
pub const ARGUMENT_BUFFER_BIND_POINT: usize = 2;
/// `kIRVertexBufferBindPoint`: argument-table buffer slot of vertex stream zero.
pub const VERTEX_BUFFER_BIND_POINT: usize = 6;
/// `kIRStageInAttributeStartIndex`: first vertex-descriptor attribute index.
pub const STAGE_IN_ATTRIBUTE_START: usize = 11;
/// Buffer slots each argument table reserves (up to and including vertex stream zero).
pub const ARGUMENT_TABLE_BUFFERS: usize = VERTEX_BUFFER_BIND_POINT + 1;

/// Sampled-image table capacity (`Texture2D[] : register(t0, space1)`).
pub const TEXTURES: u32 = 65_536;
/// Storage-buffer table capacity (`RWByteAddressBuffer[] : register(u0, space2)`).
pub const BUFFERS: u32 = 4_096;
/// Storage-image table capacity (`RWTexture2D[64] : register(u0, space3)`).
pub const STORAGE_IMAGES: u32 = 64;
/// Acceleration-structure table capacity (`RaytracingAccelerationStructure[16] : t0, space4`).
pub const ACCELERATIONS: u32 = 16;

/// One converter descriptor-table entry (`IRDescriptorTableEntry`): a GPU address for
/// buffers and acceleration-structure headers, a resource ID for textures, metadata.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TableEntry {
    /// Buffer or acceleration-structure header GPU address; zero for textures.
    pub gpu_va: u64,
    /// Texture `MTLResourceID`; zero for buffers.
    pub resource_id: u64,
    /// Buffers: byte length in the low 32 bits, untyped. Textures: min LOD clamp bits.
    pub metadata: u64,
}

impl TableEntry {
    /// An untyped (raw/structured) buffer view of `length` bytes at `gpu_va`.
    /// The documented metadata keeps only 32 length bits; longer views clamp.
    pub fn buffer(gpu_va: u64, length: u64) -> Self {
        Self {
            gpu_va,
            resource_id: 0,
            metadata: length.min(u64::from(u32::MAX)),
        }
    }
    /// A texture view with no minimum-LOD clamp.
    pub fn texture(resource_id: u64) -> Self {
        Self {
            gpu_va: 0,
            resource_id,
            metadata: u64::from(0f32.to_bits()),
        }
    }
}

/// Byte size of one table entry.
pub const ENTRY_BYTES: u64 = std::mem::size_of::<TableEntry>() as u64;

/// Top-level argument buffer of Ara's global root signature, written fresh for every
/// draw and dispatch (never modified while the GPU may read it).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TopLevel {
    /// Root parameter 0: the 32-byte root constants (`b0, space0`).
    pub root: [u32; 8],
    /// Root parameter 1: sampled-image table address.
    pub textures: u64,
    /// Root parameter 2: storage-buffer table address.
    pub buffers: u64,
    /// Root parameter 3: storage-image table address.
    pub storage_images: u64,
    /// Root parameter 4: acceleration-structure table address.
    pub accelerations: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layouts_match_the_documented_converter_encoding() {
        assert_eq!(ENTRY_BYTES, 24);
        assert_eq!(std::mem::size_of::<TopLevel>(), 64);
        assert_eq!(std::mem::offset_of!(TopLevel, textures), 32);
        assert_eq!(std::mem::offset_of!(TopLevel, accelerations), 56);
        assert_eq!(
            TableEntry::buffer(64, u64::MAX).metadata,
            u64::from(u32::MAX)
        );
        assert_eq!(TableEntry::texture(7).resource_id, 7);
    }
}
