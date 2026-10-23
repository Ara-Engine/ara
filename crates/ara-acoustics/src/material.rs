//! Frequency-dependent surface materials, using Steam Audio's three bands
//! (400 Hz, 2.5 kHz and 15 kHz centers) so baked and realtime paths agree.

use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Surface acoustics for reflection, scattering and transmission.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AcousticMaterial {
    /// Fraction of energy absorbed on reflection: low/mid/high bands.
    #[ara(default = [0.1, 0.2, 0.3], range(min = 0, max = 1))]
    pub absorption: [f32; 3],
    /// Fraction of reflected energy scattered diffusely (0 specular, 1 diffuse).
    #[ara(default = 0.05, range(min = 0, max = 1))]
    pub scattering: f32,
    /// Fraction of energy transmitted through the surface: low/mid/high bands.
    #[ara(default = [0.1, 0.05, 0.03], range(min = 0, max = 1))]
    pub transmission: [f32; 3],
}

impl Default for AcousticMaterial {
    fn default() -> Self {
        MaterialPreset::Generic.material()
    }
}

impl AcousticMaterial {
    /// True when every value is finite and within [0, 1].
    pub fn is_valid(&self) -> bool {
        self.absorption
            .iter()
            .chain(&self.transmission)
            .chain(std::iter::once(&self.scattering))
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }
}

/// Named surface materials with Steam Audio's published values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Schema, Serialize, Deserialize)]
#[ara(module = "audio")]
#[serde(rename_all = "camelCase")]
pub enum MaterialPreset {
    /// Average interior surface.
    #[default]
    Generic,
    /// Brick wall.
    Brick,
    /// Poured concrete.
    Concrete,
    /// Ceramic tile.
    Ceramic,
    /// Loose gravel.
    Gravel,
    /// Carpet on concrete.
    Carpet,
    /// Window glass.
    Glass,
    /// Plaster on brick.
    Plaster,
    /// Wooden panels.
    Wood,
    /// Sheet metal.
    Metal,
    /// Natural rock.
    Rock,
}

impl MaterialPreset {
    /// Every preset, in declaration order.
    pub const ALL: [Self; 11] = [
        Self::Generic,
        Self::Brick,
        Self::Concrete,
        Self::Ceramic,
        Self::Gravel,
        Self::Carpet,
        Self::Glass,
        Self::Plaster,
        Self::Wood,
        Self::Metal,
        Self::Rock,
    ];

    /// Lowercase name, as used in traces and diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Brick => "brick",
            Self::Concrete => "concrete",
            Self::Ceramic => "ceramic",
            Self::Gravel => "gravel",
            Self::Carpet => "carpet",
            Self::Glass => "glass",
            Self::Plaster => "plaster",
            Self::Wood => "wood",
            Self::Metal => "metal",
            Self::Rock => "rock",
        }
    }

    /// The preset's absorption, scattering and transmission.
    pub fn material(self) -> AcousticMaterial {
        let ([a0, a1, a2], scattering, [t0, t1, t2]) = match self {
            Self::Generic => ([0.10, 0.20, 0.30], 0.05, [0.100, 0.050, 0.030]),
            Self::Brick => ([0.03, 0.04, 0.07], 0.05, [0.015, 0.015, 0.015]),
            Self::Concrete => ([0.05, 0.07, 0.08], 0.05, [0.015, 0.002, 0.001]),
            Self::Ceramic => ([0.01, 0.02, 0.02], 0.05, [0.060, 0.044, 0.011]),
            Self::Gravel => ([0.60, 0.70, 0.80], 0.05, [0.031, 0.012, 0.008]),
            Self::Carpet => ([0.24, 0.69, 0.73], 0.05, [0.020, 0.005, 0.003]),
            Self::Glass => ([0.06, 0.03, 0.02], 0.05, [0.060, 0.044, 0.011]),
            Self::Plaster => ([0.12, 0.06, 0.04], 0.05, [0.056, 0.056, 0.004]),
            Self::Wood => ([0.11, 0.07, 0.06], 0.05, [0.070, 0.014, 0.005]),
            Self::Metal => ([0.20, 0.07, 0.06], 0.05, [0.200, 0.025, 0.010]),
            Self::Rock => ([0.13, 0.20, 0.24], 0.05, [0.015, 0.002, 0.001]),
        };
        AcousticMaterial {
            absorption: [a0, a1, a2],
            scattering,
            transmission: [t0, t1, t2],
        }
    }
}

/// Acoustic surface of static geometry or a collider. The physics ray adapter maps
/// hit bodies to this component; absent components read as `generic`.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[cfg_attr(feature = "ecs", derive(bevy_ecs::component::Component))]
#[ara(component, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AcousticSurface {
    /// Named material; ignored when `custom` is set.
    pub preset: MaterialPreset,
    /// Explicit material values overriding the preset.
    pub custom: Option<AcousticMaterial>,
}

impl AcousticSurface {
    /// The effective material.
    pub fn material(&self) -> AcousticMaterial {
        self.custom.unwrap_or_else(|| self.preset.material())
    }
    /// Name for traces: the preset name, or `custom`.
    pub fn name(&self) -> &'static str {
        if self.custom.is_some() {
            "custom"
        } else {
            self.preset.name()
        }
    }
}
