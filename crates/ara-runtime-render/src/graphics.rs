//! One mapping from user preferences to the renderer's actual work budgets.
use crate::Presenter;
use ara_graphics_settings::{Capabilities as UserCapabilities, Preset, Settings, Tier, Upscaler};
use ara_render_fidelity::presets::{
    Capabilities, Plan, QualityChoice, QualityPreset, RenderOptions, RenderPath,
};

/// Compose scalability groups without changing authored look or the device RT opt-in.
pub fn plan(
    settings: &Settings,
    caps: &Capabilities,
    size: [u32; 2],
    options: RenderOptions,
) -> anyhow::Result<Plan> {
    // Production user settings retain the raster scalability groups. RT opt-in
    // adds D29 hybrid queries; the experimental fidelity renderer remains available
    // through the existing explicit presenter API for development/reference work.
    let mut plan = Plan::resolve(
        choice(settings.preset),
        caps,
        size,
        RenderOptions::default(),
    )?;
    plan.rt_fallback = options.ray_tracing && !caps.ray_tracing;
    let base = plan.preset;
    let group = |tier: Option<Tier>| tier.map_or(base, preset).raster_quality();
    let groups = settings.groups;
    match &mut plan.path {
        RenderPath::Raster(raster) => {
            let shadows = group(groups.shadows);
            let view = group(groups.view_distance);
            let reflections = group(groups.reflections);
            let post = group(groups.post_processing);
            let gi = group(groups.global_illumination);
            let quality = &mut raster.quality;
            quality.shadows = shadows.shadows;
            quality.local_shadows = shadows.local_shadows;
            quality.virtual_shadows = shadows.virtual_shadows;
            quality.horizon = shadows.horizon;
            quality.lod_pixels = view.lod_pixels;
            quality.min_pixels = view.min_pixels;
            quality.terrain_detail = view.terrain_detail;
            quality.gi_budget = gi.gi_budget;
            quality.screen_gi = gi.screen_gi;
            quality.reflections = reflections.reflections;
            quality.reflection_captures = reflections.reflection_captures;
            quality.ray_queries = reflections
                .ray_queries
                .filter(|_| options.ray_tracing && caps.ray_tracing);
            quality.ambient_occlusion = post
                .ambient_occlusion
                .filter(|_| settings.ambient_occlusion);
            quality.depth_of_field = post.depth_of_field.filter(|_| settings.depth_of_field);
            quality.motion_blur = post.motion_blur.filter(|_| settings.motion_blur);
            quality.bloom = post.bloom && settings.bloom;
            let effects = group(groups.effects);
            quality.emitter_lights = effects.emitter_lights;
            // Unreal scalability keeps volumetric fog under Effects.
            quality.volumetric_fog = effects.volumetric_fog;
            quality.high_tier =
                reflections.high_tier || post.high_tier || shadows.high_tier || gi.high_tier;
            quality.upscaler = match settings.upscaler {
                Upscaler::Preset => group(groups.anti_aliasing).upscaler,
                Upscaler::Taau => ara_frame::look::Upscaler::Taau,
                Upscaler::Tsr => ara_frame::look::Upscaler::Tsr,
            };
            if let Some(scale) = settings.resolution_scale {
                raster.render_size =
                    size.map(|v| ((v as f32 * scale.clamp(0.1, 1.)).round() as u32).clamp(1, v));
            }
            quality.validate()?;
        }
        RenderPath::Fidelity(_) => {
            anyhow::bail!("production user settings require the raster/hybrid preset path")
        }
    }
    Ok(plan)
}

/// Convert a renderer-neutral choice to the renderer's preset selection API.
pub fn choice(value: Preset) -> QualityChoice {
    match value {
        Preset::Auto => QualityChoice::Auto,
        Preset::Low => QualityChoice::Preset(QualityPreset::Low),
        Preset::Medium => QualityChoice::Preset(QualityPreset::Medium),
        Preset::High => QualityChoice::Preset(QualityPreset::High),
        Preset::Epic => QualityChoice::Preset(QualityPreset::Epic),
    }
}

fn preset(tier: Tier) -> QualityPreset {
    match tier {
        Tier::Low => QualityPreset::Low,
        Tier::Medium => QualityPreset::Medium,
        Tier::High => QualityPreset::High,
        Tier::Epic => QualityPreset::Epic,
    }
}

impl Presenter {
    /// Current view preferences, including temporary development/capture overrides.
    /// None means the embedding host has not installed user settings on this view.
    pub fn graphics_settings(&self) -> Option<&Settings> {
        self.graphics.as_ref()
    }
    /// Accept validated preferences at a host control boundary. Content budgets
    /// replace residency/foliage on the next frame, retaining completed last-good output.
    pub fn set_graphics_settings(&mut self, settings: &Settings) {
        if self.graphics.as_ref() != Some(settings) {
            self.graphics_plan_key = None;
            self.choice = choice(settings.preset);
            self.graphics = Some(settings.clone());
        }
    }

    /// Query physical GPU support without creating a device or enabling ray tracing.
    pub fn graphics_capabilities(&self) -> UserCapabilities {
        let Some(renderer) = &self.renderer else {
            return UserCapabilities::default();
        };
        let info = renderer.gpu().info();
        UserCapabilities {
            device_known: true,
            ray_tracing_supported: info.ray_tracing_supported,
            device_local_mib: info.quality_memory_mib(),
            ..Default::default()
        }
    }

    /// Distance in metres to which terrain layers use their normal and metal/roughness
    /// maps (`Quality::terrain_detail`): the user's view-distance tier, or the
    /// presenter's own quality without settings.
    pub(crate) fn user_terrain_detail(&self) -> f32 {
        let tier = self.graphics.as_ref().map(|settings| {
            let caps = self.graphics_capabilities();
            settings
                .groups
                .view_distance
                .unwrap_or(settings.tier(&caps))
        });
        tier.map(preset)
            .or(self.quality())
            .unwrap_or(QualityPreset::High)
            .raster_quality()
            .terrain_detail
    }

    pub(crate) fn user_content_budget(&self) -> (u64, u32) {
        self.graphics
            .as_ref()
            .map_or((1024 * 1024 * 1024, 100), |settings| {
                let caps = self.graphics_capabilities();
                let tier = settings.groups.foliage.unwrap_or(settings.tier(&caps));
                (
                    u64::from(settings.texture_mib(&caps)) * 1024 * 1024,
                    [25, 50, 75, 100][tier.index()],
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_group_overrides_match_engine_budgets_and_rt_is_only_explicit() -> anyhow::Result<()>
    {
        use ara_graphics_settings::Groups;
        let caps = Capabilities {
            ray_tracing: true,
            device_local_mib: Some(12288),
        };
        for tier in [Tier::Low, Tier::Medium, Tier::High, Tier::Epic] {
            let settings = Settings {
                preset: Preset::Epic,
                groups: Groups {
                    view_distance: Some(tier),
                    shadows: Some(tier),
                    global_illumination: Some(tier),
                    reflections: Some(tier),
                    post_processing: Some(tier),
                    textures: Some(tier),
                    effects: Some(tier),
                    foliage: Some(tier),
                    anti_aliasing: Some(tier),
                },
                ..Default::default()
            };
            let RenderPath::Raster(raster) =
                plan(&settings, &caps, [320, 180], RenderOptions::default())?.path
            else {
                panic!("raster production plan");
            };
            let mut expected = preset(tier).raster_quality();
            expected.ray_queries = None;
            assert_eq!(raster.quality, expected);
            let RenderPath::Raster(hybrid) = plan(
                &settings,
                &caps,
                [320, 180],
                RenderOptions { ray_tracing: true },
            )?
            .path
            else {
                panic!("deterministic hybrid");
            };
            assert_eq!(hybrid.hybrid(), matches!(tier, Tier::High | Tier::Epic));
            let no_rt = Capabilities {
                ray_tracing: false,
                ..caps
            };
            let fallback = plan(
                &settings,
                &no_rt,
                [320, 180],
                RenderOptions { ray_tracing: true },
            )?;
            assert!(fallback.rt_fallback);
            assert!(matches!(fallback.path, RenderPath::Raster(raster) if !raster.hybrid()));
        }
        Ok(())
    }
    #[test]
    fn independent_groups_reach_real_quality_and_preserve_raster_default() -> anyhow::Result<()> {
        let caps = Capabilities {
            ray_tracing: true,
            device_local_mib: Some(12 * 1024),
        };
        for tier in [Tier::Low, Tier::Medium, Tier::High, Tier::Epic] {
            let settings = Settings {
                preset: Preset::Epic,
                groups: ara_graphics_settings::Groups {
                    shadows: Some(tier),
                    ..Default::default()
                },
                resolution_scale: Some(0.5),
                bloom: false,
                motion_blur: false,
                ..Default::default()
            };
            let resolved = plan(&settings, &caps, [1280, 720], RenderOptions::default())?;
            let RenderPath::Raster(raster) = resolved.path else {
                panic!("raster default");
            };
            assert_eq!(
                raster.quality.shadows,
                preset(tier).raster_quality().shadows
            );
            assert_eq!(
                raster.quality.reflections,
                QualityPreset::Epic.raster_quality().reflections
            );
            assert_eq!(raster.render_size, [640, 360]);
            assert!(!raster.quality.bloom);
            assert!(raster.quality.motion_blur.is_none() && raster.quality.ray_queries.is_none());
        }
        Ok(())
    }
}
