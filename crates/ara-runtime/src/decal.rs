//! Deferred decals (Unreal's `DecalComponent`): the reflected [`Decal`] component and
//! its extraction view. Renderers project the decal-domain material along the
//! entity's local -Z onto the opaque surfaces inside its box, before lighting.
use ara_schema::{AssetRef, Color, Schema};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Projects a decal-domain material onto the opaque surfaces inside a box: the box
/// is `size` metres around the entity, projecting along its local -Z (surfaces facing
/// +Z receive it). Writes base colour, normal, roughness, metallic, emissive and
/// ambient occlusion with per-channel opacity before lighting, so every light, shadow,
/// reflection and GI term sees it. Translucent surfaces and renderers whose
/// `MeshRenderer.receivesDecals` is off do not receive.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Decal {
    /// Decal-domain material graph or material instance (`@path#id`); without one
    /// the decal draws nothing.
    pub material: Option<AssetRef>,
    /// Box size in metres: width (local X), height (local Y) and projection depth
    /// (local Z), centred on the entity. Entity scale multiplies it.
    #[ara(default = [1, 1, 1], unit = Meters, range(min = 0.001))]
    pub size: [f32; 3],
    /// Linear multiplier on the material's base colour; alpha scales every channel's
    /// opacity (Unreal's DecalColor).
    pub color: Color,
    /// How strongly the decal replaces the surface's base colour.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub base_color_opacity: f32,
    /// How strongly the decal's normal (blended in the surface's tangent frame)
    /// replaces the surface normal.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub normal_opacity: f32,
    /// How strongly the decal replaces roughness.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub roughness_opacity: f32,
    /// How strongly the decal replaces metalness.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub metallic_opacity: f32,
    /// How strongly the decal's specular level applies. The G-buffer has no specular
    /// channel yet (dielectric F0 is fixed at 4%); the value is kept for it.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub specular_opacity: f32,
    /// How strongly the decal's emission replaces the surface's.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub emissive_opacity: f32,
    /// How strongly the decal replaces material ambient occlusion.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub occlusion_opacity: f32,
    /// Camera distance where the decal starts fading out (metres; with
    /// `fadeEndDistance` zero it never fades by distance).
    #[ara(unit = Meters, range(min = 0))]
    pub fade_start_distance: f32,
    /// Camera distance where the decal has faded out and is no longer drawn.
    #[ara(unit = Meters, range(min = 0))]
    pub fade_end_distance: f32,
    /// Surfaces turned more than this many degrees from the projection axis start
    /// fading (stretching along steep sides stays hidden).
    #[ara(default = 60, unit = Degrees, range(min = 0, max = 180))]
    pub fade_angle_start: f32,
    /// Surfaces turned this many degrees or more no longer receive the decal.
    #[ara(default = 80, unit = Degrees, range(min = 0, max = 180))]
    pub fade_angle_end: f32,
    /// Draw order among overlapping decals: higher draws on top (Unreal's SortOrder).
    pub sort_order: i32,
    /// Below this share of the screen height the decal fades out and is skipped
    /// (Unreal's FadeScreenSize); zero always draws.
    #[ara(default = 0.002, range(min = 0, max = 1))]
    pub fade_screen_size: f32,
}

impl Default for Decal {
    fn default() -> Self {
        Self {
            material: None,
            size: [1.0; 3],
            color: Color([1.0; 4]),
            base_color_opacity: 1.0,
            normal_opacity: 1.0,
            roughness_opacity: 1.0,
            metallic_opacity: 1.0,
            specular_opacity: 1.0,
            emissive_opacity: 1.0,
            occlusion_opacity: 1.0,
            fade_start_distance: 0.0,
            fade_end_distance: 0.0,
            fade_angle_start: 60.0,
            fade_angle_end: 80.0,
            sort_order: 0,
            fade_screen_size: 0.002,
        }
    }
}

impl Decal {
    /// Opacities in the renderer's channel order: base colour, normal, roughness,
    /// metallic, specular, emissive and ambient occlusion, each times `color` alpha.
    pub fn opacities(&self) -> [f32; 7] {
        [
            self.base_color_opacity,
            self.normal_opacity,
            self.roughness_opacity,
            self.metallic_opacity,
            self.specular_opacity,
            self.emissive_opacity,
            self.occlusion_opacity,
        ]
        .map(|o| (o * self.color.0[3]).clamp(0.0, 1.0))
    }
}

/// A decal for presentation at its interpolated pose.
#[derive(Debug, Clone)]
pub struct DecalView {
    /// Stable render identity (`stable_id` of the address).
    pub entity: u64,
    /// Stable document address.
    pub address: String,
    /// Capture legend label: scene name and address.
    pub label: String,
    /// The box's unit cube (-0.5..0.5 on every axis) to world: the entity pose times
    /// `size`.
    pub world: glam::Mat4,
    /// The authored component.
    pub decal: Decal,
    /// The resolved decal-domain material; `None` until it loads (nothing draws).
    pub material: Option<Arc<crate::material::MaterialAsset>>,
}
