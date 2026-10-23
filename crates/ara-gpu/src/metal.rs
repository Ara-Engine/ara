//! Metal 4 implementation point. The S1 Mac session supplies native encoding.
//! Implement `Device` with the same generation, queue and retirement semantics.
//! Converter tables are referenced by a fresh root buffer bound through an
//! MTL4ArgumentTable; indirect resources must be included in residency sets.

/// Metal backend marker; no Vulkan fallback is selected on Apple platforms.
pub struct MetalBackend;
