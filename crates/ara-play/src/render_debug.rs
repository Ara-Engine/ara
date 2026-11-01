//! Rendering diagnosis switches a Play start may carry (the editor's View >
//! Rendering Debug); players apply them to every frame of that world.
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Effects to turn off so one artifact (flicker, aliasing, darkening) can be traced
/// to its source. Omitted fields keep the effect on; full resolution is opt-in.
#[derive(Clone, Copy, Debug, PartialEq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RenderDebug {
    /// Screen-space ambient occlusion (default true).
    #[ara(default = true)]
    pub ambient_occlusion: bool,
    /// Sun and local-light shadows (default true).
    #[ara(default = true)]
    pub shadows: bool,
    /// Soft shadows (default true): penumbrae sized by each light's shape; off, every
    /// light shadows as a point (the narrowest filter).
    #[ara(default = true)]
    pub soft_shadows: bool,
    /// The detailed (froxel) volumetric fog near the camera (default true); off, fog
    /// keeps only its coarse far grid, as on Low.
    #[ara(default = true)]
    pub volumetric_fog: bool,
    /// The sub-pixel camera jitter temporal anti-aliasing accumulates (default true);
    /// off, edges alias but hold still.
    #[ara(default = true)]
    pub temporal_jitter: bool,
    /// Render at display resolution instead of the preset's upscaled scale (default false).
    pub full_resolution: bool,
    /// What the lit image shows of reflections (default `lit`).
    pub reflections: ReflectionView,
    /// Screen-space reflections (default true); off, captures, probes and sky reflect.
    #[ara(default = true)]
    pub screen_reflections: bool,
    /// Reflection captures (default true); off, probes and sky reflect.
    #[ara(default = true)]
    pub reflection_captures: bool,
    /// Probe global illumination (default true); off, the sky lights everything.
    #[ara(default = true)]
    pub global_illumination: bool,
    /// Render the path-traced ground truth instead (default off): unbiased and
    /// undenoised, on GPUs with hardware ray tracing whatever the project's
    /// ray-tracing setting, to judge the real-time lighting against.
    pub reference: Option<ReferenceTrace>,
    /// Lock exposure at this EV100 (default off): manual exposure without
    /// compensation, over every post-process volume, so images from different
    /// renderers and presets compare at the same brightness.
    #[ara(range(min = -10, max = 24))]
    pub exposure: Option<f32>,
    /// The debug view drawn instead of Lit, in the window and in captures (default
    /// `lit`; `ara capture --view`).
    pub view: DebugView,
}

/// The renderer's debug views (`ara_render::capture::ViewMode`): camelCase names, and
/// its snake_case names (the editor sends `ara_render::RenderDebug`) as aliases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "play")]
#[serde(rename_all = "camelCase")]
pub enum DebugView {
    /// The lit image.
    #[default]
    Lit,
    /// Linear base colour.
    Albedo,
    /// World normals.
    Normals,
    /// Reversed-Z depth.
    Depth,
    /// Roughness (red) and metalness (green).
    #[serde(alias = "roughness_metal")]
    RoughnessMetal,
    /// Pseudocolour entity IDs.
    #[serde(alias = "entity_ids")]
    EntityIds,
    /// Cascade selection times shadow visibility.
    #[serde(alias = "shadow_cascades")]
    ShadowCascades,
    /// Triangle edges.
    Wireframe,
    /// The lit image with triangle edges.
    #[serde(alias = "lit_wireframe")]
    LitWireframe,
    /// Diffuse indirect light alone.
    #[serde(alias = "gi_diffuse")]
    GiDiffuse,
    /// Fragments per pixel before depth rejection.
    Overdraw,
    /// Vertex colour RGB as stored.
    #[serde(alias = "vertex_color")]
    VertexColor,
    /// Vertex colour red alone.
    #[serde(alias = "vertex_color_red")]
    VertexColorRed,
    /// Vertex colour green alone.
    #[serde(alias = "vertex_color_green")]
    VertexColorGreen,
    /// Vertex colour blue alone.
    #[serde(alias = "vertex_color_blue")]
    VertexColorBlue,
    /// Vertex colour alpha alone.
    #[serde(alias = "vertex_color_alpha")]
    VertexColorAlpha,
    /// Deferred decals per pixel.
    #[serde(alias = "decal_coverage")]
    DecalCoverage,
    /// Local-light shading cost per pixel, on the overdraw heat legend: each light
    /// reaching the surface counts one, an area light three, a shadowed one one more.
    #[serde(alias = "light_complexity")]
    LightComplexity,
    /// Soft-shadow penumbra half-widths of the sun and shadowed local lights, on the
    /// overdraw heat legend as sqrt(width / 1 cm): 1 cm blue to 2.56 m red; grey
    /// where no blocker is near.
    #[serde(alias = "shadow_penumbra")]
    ShadowPenumbra,
}

/// The path-traced reference's budget ([`RenderDebug::reference`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "play")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ReferenceTrace {
    /// Independent samples per pixel (default 1024; 8 accumulate per frame).
    #[ara(default = 1024, range(min = 8, max = 65536))]
    pub samples: u32,
    /// Scattering events per path (default 8).
    #[ara(default = 8, range(min = 1, max = 16))]
    pub bounces: u32,
}

impl Default for ReferenceTrace {
    fn default() -> Self {
        Self {
            samples: 1024,
            bounces: 8,
        }
    }
}

/// The reflection debug views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "play")]
#[serde(rename_all = "camelCase")]
pub enum ReflectionView {
    /// The lit image.
    #[default]
    Lit,
    /// Only the reflected (indirect specular) light, exposed like the lit image.
    Reflections,
    /// Each pixel coloured by where its reflection comes from: on-screen trace green,
    /// capture trace blue, box-projected captures cyan, probes orange, sky magenta;
    /// surfaces too rough to trace dimmed.
    Sources,
}

impl Default for RenderDebug {
    fn default() -> Self {
        Self {
            ambient_occlusion: true,
            shadows: true,
            soft_shadows: true,
            volumetric_fog: true,
            temporal_jitter: true,
            full_resolution: false,
            reflections: ReflectionView::Lit,
            screen_reflections: true,
            reflection_captures: true,
            global_illumination: true,
            reference: None,
            exposure: None,
            view: DebugView::Lit,
        }
    }
}
