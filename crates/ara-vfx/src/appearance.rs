use serde::{Deserialize, Serialize};

/// Offline procedural sprite sheets, with sixteen smoothly blended frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpriteTexture {
    /// Analytic soft particle without a texture.
    None,
    /// Turbulent flame tongues with a hot core.
    Fire,
    /// Advected cloud density with tangent-space lighting gradients.
    Smoke,
    /// Tapered luminous streak.
    Spark,
    /// Expanding ground-plane ripple.
    Ring,
    /// Curved wisps for magical energy.
    Wisp,
}
/// Linear-light compositing policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    /// Premultiplied color plus source-over transmittance.
    Alpha,
    /// Premultiplied radiance added without obscuring the scene.
    Additive,
}
/// Sprite basis in world space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    /// Camera-facing quad.
    Camera,
    /// Camera-facing length axis follows projected particle velocity.
    Velocity,
    /// Horizontal decal-like quad above the ground.
    Ground,
}
