//! Metal 4 implementation point. The S1 Mac session supplies native encoding.
//! Implement `Device` with the same generation, queue and retirement semantics.
//! Converter tables are referenced by a fresh root buffer bound through an
//! MTL4ArgumentTable; indirect resources must be included in residency sets.

/// Metal backend marker; no Vulkan fallback is selected on Apple platforms.
pub struct MetalBackend;

impl MetalBackend {
    /// Shared surfaces ([`crate::shared`], D19 embedded Play) are not implemented on
    /// Metal yet (IOSurface or `MTLSharedTextureHandle` are the candidates).
    pub fn shared_surfaces(&self) -> crate::Result<()> {
        Err(crate::GpuError::External(
            "shared surfaces are unsupported on the Metal backend".into(),
        ))
    }
}
