use serde::{Deserialize, Serialize};

/// A renderer budget tier, independently selectable for each scalability group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    /// Smallest work budget.
    Low,
    /// Balanced work budget.
    Medium,
    /// Detailed realtime rendering.
    #[default]
    High,
    /// Largest work budget; never selected automatically without measurements.
    Epic,
}

impl Tier {
    /// Index into the four engine budget tables, cheapest first.
    pub const fn index(self) -> usize {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Epic => 3,
        }
    }
}

/// Overall quality selection. Explicit group overrides survive a preset change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    /// Choose from device memory, or the last explicit benchmark result.
    #[default]
    Auto,
    /// Low raster budgets.
    Low,
    /// Medium raster budgets.
    Medium,
    /// High raster budgets.
    High,
    /// Epic raster budgets.
    Epic,
}

/// Native game window policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowMode {
    /// Resizable native window.
    Windowed,
    /// Desktop fullscreen without a monitor mode switch.
    #[default]
    Borderless,
    /// Exclusive fullscreen using a supported monitor mode.
    Fullscreen,
}

/// Supported temporal reconstruction methods; both retain temporal antialiasing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Upscaler {
    /// Use the anti-aliasing group's preset method.
    #[default]
    Preset,
    /// Temporal antialiasing and upsampling.
    Taau,
    /// Ara temporal super resolution.
    Tsr,
}

/// Independent groups. `None` inherits the overall preset, including Auto.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Groups {
    /// Screen-size culling and geometric LOD error.
    pub view_distance: Option<Tier>,
    /// Cascades, far maps, local and virtual shadows.
    pub shadows: Option<Tier>,
    /// Probe relight budget.
    pub global_illumination: Option<Tier>,
    /// SSR tracing and reflection capture budgets.
    pub reflections: Option<Tier>,
    /// Bloom, AO, depth of field and motion blur budgets.
    pub post_processing: Option<Tier>,
    /// Texture streaming residency budget.
    pub textures: Option<Tier>,
    /// Emissive light budget.
    pub effects: Option<Tier>,
    /// Stable foliage density (25/50/75/100 percent).
    pub foliage: Option<Tier>,
    /// Temporal antialiasing/reconstruction method.
    pub anti_aliasing: Option<Tier>,
}

/// Per-user graphics preferences, separate from authored look and gameplay state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Settings {
    /// Overall quality; group overrides are retained.
    pub preset: Preset,
    /// Overrides for individual scalability groups.
    pub groups: Groups,
    /// Optional explicit render/display scale, clamped to 0.1..1; None uses preset.
    pub resolution_scale: Option<f32>,
    /// Explicit reconstruction method, or the anti-aliasing group's method.
    pub upscaler: Upscaler,
    /// Allow bloom when enabled by the post-processing group.
    pub bloom: bool,
    /// Allow authored motion blur within the group's budget.
    pub motion_blur: bool,
    /// Allow authored depth of field within the group's budget.
    pub depth_of_field: bool,
    /// Allow ambient occlusion within the group's budget.
    pub ambient_occlusion: bool,
    /// Optional texture residency cap in MiB; None uses the textures group.
    pub texture_budget_mib: Option<u32>,
    /// Native window mode; embedded/headless players retain their owner surface.
    pub window_mode: WindowMode,
    /// Requested display width in physical pixels, 320..8192.
    pub width: u32,
    /// Requested display height in physical pixels, 180..8192.
    pub height: u32,
    /// Refresh rate in millihertz; zero selects the desktop/default rate.
    pub refresh_rate_millihz: u32,
    /// Stable platform display ID; zero selects the current/primary monitor.
    pub monitor: u32,
    /// FIFO presentation synchronization.
    pub vsync: bool,
    /// Presentation FPS limit, 0 for uncapped, otherwise 15..1000. Never a tick rate.
    pub frame_rate_cap: u32,
    /// Request HDR10 output; unavailable output is rejected on Apply.
    pub hdr: bool,
    /// Explicit hardware RT opt-in. Off by default; changing it requires restart.
    pub ray_tracing: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preset: Preset::Auto,
            groups: Groups::default(),
            resolution_scale: None,
            upscaler: Upscaler::Preset,
            bloom: true,
            motion_blur: true,
            depth_of_field: true,
            ambient_occlusion: true,
            texture_budget_mib: None,
            window_mode: WindowMode::Borderless,
            width: 1280,
            height: 720,
            refresh_rate_millihz: 0,
            monitor: 0,
            vsync: true,
            frame_rate_cap: 0,
            hdr: false,
            ray_tracing: false,
        }
    }
}

/// One enumerated exclusive display mode, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayMode {
    /// Platform display ID.
    pub monitor: u32,
    /// Horizontal pixels.
    pub width: u32,
    /// Vertical pixels.
    pub height: u32,
    /// Refresh rate in millihertz.
    pub refresh_rate_millihz: u32,
}

/// Capabilities of this player surface, updated when the device/display changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Capabilities {
    /// Verified physical GPU support, independent of the current opt-in.
    pub ray_tracing_supported: bool,
    /// Hardware has been queried; false before first device creation.
    pub device_known: bool,
    /// Device-local memory, or None for shared/unknown memory.
    pub device_local_mib: Option<u64>,
    /// This player owns a mutable display surface (false in headless/embedded play).
    pub display_available: bool,
    /// This surface advertises HDR10 precision and transfer.
    pub hdr_supported: bool,
    /// Enumerated fullscreen modes; empty for headless/embedded players.
    pub modes: Vec<DisplayMode>,
}
