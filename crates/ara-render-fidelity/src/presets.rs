//! Quality presets: one user-facing choice mapped onto renderer selection, render
//! resolution and work budgets. Hosts resolve a [`Plan`] once per view (and again on
//! resize or device change); every plan keeps temporal antialiasing on.
//!
//! Hardware ray tracing is off on every preset unless the project or game opts in
//! ([`RenderOptions::ray_tracing`]), as in Unreal: by default every preset is raster
//! ([`RasterPlan`], `ara_frame::look::Quality`) and traces no rays.
use anyhow::{Result, bail};
use ara_frame::look::{
    AmbientOcclusion, DepthOfFieldQuality, FarShadows, LocalShadows, MotionBlurQuality, Quality,
    RayQueries, ReflectionCaptures, Reflections, ScreenGi, Shadows, VirtualShadows,
    VolumetricFogQuality,
};
use ara_gpu::GpuDevice;

/// User-facing quality tier. D27: every tier is raster by default
/// ([`QualityPreset::raster_quality`]); with [`RenderOptions::ray_tracing`] on an RT
/// device, Medium and above use the ray-traced fidelity renderer instead (ReSTIR
/// DI/GI with grid-resolution reuse; D26 budgets at 1440p on an RTX 4070 SUPER:
/// High GPU <= 6 ms, Medium <= 3.5, Epic <= 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualityPreset {
    /// Raster: 0.45 render scale, two 512 shadow cascades, probe GI, coarser LODs,
    /// no bloom (the cheapest look).
    Low,
    /// Raster: 0.55 render scale, three cascades and bloom. Ray traced: ReSTIR at
    /// 0.4 scale, rough reflections from the probe field above roughness 0.35.
    Medium,
    /// Raster: four 2048 cascades to 150 m. Ray traced: ReSTIR at half scale,
    /// paths ending in the probe field after one traced event.
    High,
    /// Raster: four cascades to 250 m, the largest GI budget. Ray traced: ReSTIR
    /// at two-thirds scale with two traced events before the probe field.
    Epic,
}

/// Screen probe GI tuned for High (D32); off by default until it matches probe GI alone
/// on every test scene.
pub const SCREEN_GI_HIGH: ScreenGi = ScreenGi {
    tile: 16,
    steps: 32,
    distance: 30.,
};
/// Screen probe GI tuned for Epic: 16-pixel tiles as on High (8 cost 3.4 ms at 1440p),
/// longer traces.
pub const SCREEN_GI_EPIC: ScreenGi = ScreenGi {
    tile: 16,
    steps: 48,
    distance: 50.,
};

impl QualityPreset {
    /// Every preset, cheapest first.
    pub const ALL: [QualityPreset; 4] = [Self::Low, Self::Medium, Self::High, Self::Epic];

    /// Provisional raster auto tier: discrete adapters with at least 8 GiB start
    /// at High, 6 GiB at Medium, and shared-memory/entry adapters at Low. Validate
    /// these tiers against measured budgets; Epic requires measured headroom.
    /// Ray-tracing enablement never lowers the raster tier.
    pub fn detect_raster(capabilities: &Capabilities) -> Self {
        match capabilities.device_local_mib {
            Some(mib) if mib >= 8 * 1024 => Self::High,
            Some(mib) if mib >= 6 * 1024 => Self::Medium,
            _ => Self::Low,
        }
    }

    /// Hardware-selected default for the D27 raster path, including editor labels.
    pub fn detect(capabilities: &Capabilities) -> Self {
        Self::detect_raster(capabilities)
    }

    /// The default preset for the opt-in ray-traced renderer: Epic is opt-in, RT
    /// devices start at High (Medium with under 8 GiB of device-local memory), and
    /// others use Low.
    fn detect_ray_traced(capabilities: &Capabilities) -> Self {
        if !capabilities.ray_tracing {
            Self::Low
        } else if capabilities
            .device_local_mib
            .is_some_and(|mib| mib < 8 * 1024)
        {
            Self::Medium
        } else {
            Self::High
        }
    }

    /// Lower-case name, as accepted by `--quality` and project settings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Epic => "epic",
        }
    }
}

impl std::fmt::Display for QualityPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A requested quality: a fixed preset, or detection from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QualityChoice {
    /// Pick with [`QualityPreset::detect`].
    #[default]
    Auto,
    /// Use this preset, downgraded only when the device cannot run it.
    Preset(QualityPreset),
}

impl std::str::FromStr for QualityChoice {
    type Err = anyhow::Error;

    /// Parses `auto`, `low`, `medium`, `high` or `epic` (case-insensitive).
    fn from_str(s: &str) -> Result<Self> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        QualityPreset::ALL
            .into_iter()
            .find(|p| s.eq_ignore_ascii_case(p.name()))
            .map(Self::Preset)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown quality `{s}`; expected auto, low, medium, high or epic")
            })
    }
}

impl std::fmt::Display for QualityChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Preset(p) => p.fmt(f),
        }
    }
}

impl From<ara_components::render::QualitySetting> for QualityChoice {
    fn from(setting: ara_components::render::QualitySetting) -> Self {
        use ara_components::render::QualitySetting as Q;
        match setting {
            Q::Auto => Self::Auto,
            Q::Low => Self::Preset(QualityPreset::Low),
            Q::Medium => Self::Preset(QualityPreset::Medium),
            Q::High => Self::Preset(QualityPreset::High),
            Q::Epic => Self::Preset(QualityPreset::Epic),
        }
    }
}

/// What the device can run, read once after device creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Hardware ray queries (the RT tier).
    pub ray_tracing: bool,
    /// Device-local memory in MiB (dedicated, or Apple unified memory); absent for
    /// shared-memory integrated or unknown adapters, which use the conservative raster
    /// tier.
    pub device_local_mib: Option<u64>,
}

/// Whether `gpu`'s ray queries are verified for Ara's ray-traced paths: the D29
/// hybrid, the RT fidelity tier and the ray-traced probe-GI bake. Every RT device's
/// are, Metal's included since the RIS reservoir fix (its RT gallery matches the
/// Vulkan goldens); `ARA_METAL_RT=0` keeps a Mac on raster to compare against.
pub fn verified_ray_tracing(gpu: &GpuDevice) -> bool {
    gpu.info().ray_tracing
        && !(cfg!(target_os = "macos")
            && std::env::var_os("ARA_METAL_RT").is_some_and(|value| value == "0"))
}

impl Capabilities {
    /// Reads the capabilities of an opened device (ray tracing only where verified:
    /// [`verified_ray_tracing`]).
    pub fn of(gpu: &GpuDevice) -> Self {
        Self {
            ray_tracing: verified_ray_tracing(gpu),
            device_local_mib: gpu.info().quality_memory_mib(),
        }
    }
}

/// Raster work budgets selected by a preset: the look's per-preset quality and
/// resolutions. Apply with `Renderer::set_quality`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterPlan {
    /// Render resolution of the scene passes: the display scaled by
    /// [`QualityPreset::raster_scale`], temporally upscaled to the display.
    pub render_size: [u32; 2],
    /// Display resolution.
    pub display_size: [u32; 2],
    /// Shadows, probe GI budget and post effects. `quality.ray_queries` (D29's
    /// hybrid reflections) is set only under the ray-tracing opt-in on a device
    /// with hardware ray queries; default plans are pure raster.
    pub quality: Quality,
}

impl RasterPlan {
    /// Whether this plan adds hardware ray queries to the raster renderer (D29's
    /// hybrid path, only under the ray-tracing opt-in).
    pub fn hybrid(&self) -> bool {
        self.quality.ray_queries.is_some()
    }
}

/// Renderer selection options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderOptions {
    /// Opt in to hardware ray tracing on devices with verified ray queries: the
    /// ray-traced fidelity renderer for Medium and above. Off (the default), every
    /// preset is raster and traces no rays, whatever the GPU.
    pub ray_tracing: bool,
}

/// The display the preset budgets were set at (D26/D27: 2560x1440); smaller
/// displays may keep that pixel count ([`QualityPreset::raster_scale_for`]).
pub const BUDGET_DISPLAY: [u32; 2] = [2560, 1440];

impl QualityPreset {
    /// Raster render scale per display axis: the scene renders at this fraction of
    /// the display and the temporal resolve reconstructs the display resolution
    /// (TAAU, `Renderer::resize_upscaled`). Chosen with the budgets below for the
    /// D26 frame targets at 1440p (Low 2, Medium 3.5, High 6, Epic 10 ms, p50 and
    /// p95 on Outpost, Hangar and Showcase).
    pub fn raster_scale(self) -> f32 {
        match self {
            Self::Low => 0.45,
            Self::Medium => 0.55,
            // Ara TSR (High/Epic) reconstructs from fewer pixels at TAAU's quality:
            // 0.6 / 0.7 match TAAU at 2/3 / 0.77 within ~0.002 FLIP of supersampled
            // truth, cost the same, and pass the no-visible-noise gate TAAU fails.
            Self::High => 0.6,
            Self::Epic => 0.7,
        }
    }

    /// Raster render scale per display axis for `display` by pixel budget (Unreal's
    /// display-resolution screen percentage): [`Self::raster_scale`] at 2560x1440 and
    /// above; below it, the scale that renders as many pixels as the preset does at
    /// 1440p, up to native. Editor Game views use it (`Presenter::set_pixel_budget`):
    /// a panel costs no more than a 1440p frame and is sharp instead of upscaling a few
    /// hundred lines (High: native up to ~1.3 MP). Plans keep [`Self::raster_scale`].
    pub fn raster_scale_for(self, display: [u32; 2]) -> f32 {
        let scale = self.raster_scale();
        let pixels = |s: [u32; 2]| s[0] as f32 * s[1] as f32;
        let budget = scale * scale * pixels(BUDGET_DISPLAY);
        (budget / pixels(display).max(1.)).sqrt().clamp(scale, 1.)
    }

    /// Raster budgets for this preset (D27 record targets at 2560x1440 on an RTX
    /// 4070 SUPER: Low 1.0 ms, Medium 1.75, High 3.0, Epic 5.0).
    pub fn raster_quality(self) -> Quality {
        // Far maps keep distant hills and buildings self-shadowed past the
        // cascades on every preset.
        let shadows = |cascades, resolution, distance, far: (u32, f32)| Shadows {
            cascades,
            resolution,
            distance,
            far: Some(FarShadows {
                resolution: far.0,
                distance: far.1,
            }),
        };
        let ssr = |max_roughness, distance, steps| {
            Some(Reflections {
                max_roughness,
                distance,
                steps,
            })
        };
        let ao = |slices, steps| {
            Some(AmbientOcclusion {
                radius: 1.,
                slices,
                steps,
            })
        };
        // Lens effects run only while a post-process volume asks for them: depth of
        // field from Low (a cheap gather), motion blur from Medium.
        let dof = |samples, max_radius| {
            Some(DepthOfFieldQuality {
                samples,
                max_radius,
            })
        };
        let motion = |samples, max_length| {
            Some(MotionBlurQuality {
                samples,
                max_length,
            })
        };
        // D29 hybrid ray queries (High/Epic): mirror and glossy reflections below
        // the roughness cut, ray-traced occlusion of rougher ones up to the
        // second cut. `Plan::resolve` keeps them only under the ray-tracing
        // opt-in on RT devices.
        let rays = |reflection_roughness, rough_roughness, distance| {
            Some(RayQueries {
                reflection_roughness,
                rough_roughness,
                distance,
            })
        };
        // The near froxel volume of volumetric fog: render pixels per froxel and slices
        // (Unreal's grid pixel size and grid size Z), then the per-pixel local-light
        // scatter's render pixels per texel side and equiangular samples per light.
        let volumetric = |tile, slices, light_scale, light_samples| {
            Some(VolumetricFogQuality {
                tile,
                slices,
                light_scale,
                light_samples,
            })
        };
        match self {
            // LOD error and the cull size are in render pixels (about two
            // display pixels each at Low's scale).
            Self::Low => Quality {
                // 1024-texel cascades: at 512 the near cascade's texels (several
                // centimetres) left floor shadows blobby next to the references.
                shadows: shadows(2, 1024, 60., (1024, 600.)),
                ambient_occlusion: None,
                reflections: None,
                gi_budget: 256,
                bloom: false,
                emitter_lights: 4,
                lod_pixels: 6.,
                min_pixels: 6.,
                high_tier: false,
                upscaler: ara_frame::look::Upscaler::Taau,
                reflection_captures: None,
                terrain_detail: 0.,
                local_shadows: None,
                horizon: Some(1024),
                virtual_shadows: None,
                depth_of_field: dof(12, 0.01),
                motion_blur: None,
                ray_queries: None,
                // The coarse fog grid alone (still shadowed).
                volumetric_fog: None,
                screen_gi: None,
            },
            Self::Medium => Quality {
                shadows: shadows(3, 1024, 100., (1024, 1000.)),
                ambient_occlusion: ao(2, 4),
                reflections: ssr(0.4, 20., 16),
                gi_budget: 1024,
                bloom: true,
                emitter_lights: 6,
                lod_pixels: 3.,
                min_pixels: 3.,
                high_tier: false,
                upscaler: ara_frame::look::Upscaler::Taau,
                reflection_captures: None,
                terrain_detail: 40.,
                local_shadows: Some(LocalShadows {
                    lights: 2,
                    resolution: 256,
                    redraws: 6,
                    in_gi_and_fog: true,
                }),
                horizon: Some(2048),
                virtual_shadows: None,
                depth_of_field: dof(20, 0.015),
                motion_blur: motion(8, 24.),
                ray_queries: None,
                volumetric_fog: volumetric(16, 48, 2, 12),
                screen_gi: None,
            },
            Self::High => Quality {
                shadows: shadows(4, 2048, 150., (2048, 1500.)),
                // Three steps per slice side, with far samples occluding less
                // (look-ao.slang aoBitmaskMain): six steps over-darkened against Cycles
                // and cost 0.07 ms more at 1440p.
                ambient_occlusion: ao(2, 3),
                reflections: ssr(0.6, 30., 32),
                gi_budget: 2048,
                bloom: true,
                emitter_lights: 8,
                lod_pixels: 1.,
                min_pixels: 1.5,
                high_tier: true,
                upscaler: ara_frame::look::Upscaler::Tsr,
                reflection_captures: reflection_captures(8),
                terrain_detail: 150.,
                local_shadows: Some(LocalShadows {
                    lights: 4,
                    resolution: 512,
                    redraws: 12,
                    in_gi_and_fog: true,
                }),
                horizon: Some(2048),
                virtual_shadows: Some(VirtualShadows {
                    pages: 1024,
                    budget: 32,
                    bias: 0.,
                }),
                depth_of_field: dof(32, 0.02),
                motion_blur: motion(12, 32.),
                ray_queries: rays(0.3, 0.6, 1500.),
                volumetric_fog: volumetric(12, 64, 2, 16),
                // Off by default (D32): enabled, six of the ten test scenes read further
                // from the path tracer than probe GI alone (rough lobes at grazing views,
                // rough-metal ceilings). Tuned settings: SCREEN_GI_HIGH.
                screen_gi: None,
            },
            Self::Epic => Quality {
                shadows: shadows(4, 2048, 250., (2048, 2000.)),
                ambient_occlusion: ao(3, 8),
                reflections: ssr(0.6, 50., 48),
                gi_budget: 4096,
                bloom: true,
                emitter_lights: 16,
                lod_pixels: 1.,
                min_pixels: 1.,
                high_tier: true,
                upscaler: ara_frame::look::Upscaler::Tsr,
                reflection_captures: reflection_captures(16),
                terrain_detail: 300.,
                local_shadows: Some(LocalShadows {
                    lights: 8,
                    resolution: 512,
                    redraws: 24,
                    in_gi_and_fog: true,
                }),
                horizon: Some(2048),
                virtual_shadows: Some(VirtualShadows {
                    pages: 2048,
                    budget: 64,
                    bias: 0.,
                }),
                depth_of_field: dof(48, 0.025),
                motion_blur: motion(16, 32.),
                ray_queries: rays(0.5, 0.8, 2000.),
                volumetric_fog: volumetric(8, 96, 2, 24),
                // Off by default, as on High; tuned settings: SCREEN_GI_EPIC.
                screen_gi: None,
            },
        }
    }
}

/// A resolved preset for one view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// The preset that runs.
    pub preset: QualityPreset,
    /// The preset that was asked for, before any device downgrade.
    pub requested: QualityPreset,
    /// Resolutions and work budgets.
    pub raster: RasterPlan,
    /// Ray tracing was requested on a device without verified ray queries
    /// ([`verified_ray_tracing`]); hosts report ARA6008 so the declined opt-in is
    /// never silent.
    pub rt_fallback: bool,
}

/// High/Epic local reflection captures (512-texel tiles, `captures` at most). Authored
/// reflection probes always bake; scenes without any place them automatically where
/// the probe volume saw enclosed open space, and reflections trace through them
/// (`ARA_REFLECTION_CAPTURES=0` turns automatic placement off, for A/B captures).
fn reflection_captures(captures: u32) -> Option<ReflectionCaptures> {
    Some(ReflectionCaptures {
        resolution: 512,
        captures,
        automatic: std::env::var_os("ARA_REFLECTION_CAPTURES").is_none_or(|v| v != "0"),
    })
}

impl Plan {
    /// Resolves a choice for a device and display size. Every preset runs on every
    /// device, so nothing is downgraded; ray queries stay off unless `options` opts in
    /// to ray tracing on a capable device.
    pub fn resolve(
        choice: QualityChoice,
        capabilities: &Capabilities,
        display_size: [u32; 2],
        options: RenderOptions,
    ) -> Result<Self> {
        if display_size.contains(&0) || display_size.iter().any(|v| *v > 8192) {
            bail!("Invalid display size {display_size:?}");
        }
        let traced = options.ray_tracing && capabilities.ray_tracing;
        let requested = match choice {
            QualityChoice::Auto if traced => QualityPreset::detect_ray_traced(capabilities),
            QualityChoice::Auto => QualityPreset::detect_raster(capabilities),
            QualityChoice::Preset(p) => p,
        };
        let preset = requested;
        let scaled = |scale: f32| display_size.map(|v| ((v as f32 * scale).round() as u32).max(1));
        // D29's hybrid ray queries trace only under the opt-in on an RT device.
        let quality = Quality {
            ray_queries: preset.raster_quality().ray_queries.filter(|_| traced),
            ..preset.raster_quality()
        };
        let raster = RasterPlan {
            render_size: scaled(preset.raster_scale()),
            display_size,
            quality,
        };
        Ok(Self {
            preset,
            requested,
            raster,
            rt_fallback: options.ray_tracing && !capabilities.ray_tracing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RT: Capabilities = Capabilities {
        ray_tracing: true,
        device_local_mib: Some(12 * 1024),
    };

    #[test]
    fn detection_follows_capabilities() {
        assert_eq!(QualityPreset::detect(&RT), QualityPreset::High);
        let small = Capabilities {
            device_local_mib: Some(6 * 1024),
            ..RT
        };
        assert_eq!(QualityPreset::detect(&small), QualityPreset::Medium);
        let raster = Capabilities {
            ray_tracing: false,
            device_local_mib: None,
        };
        assert_eq!(QualityPreset::detect(&raster), QualityPreset::Low);
        for (mib, expected) in [
            (4 * 1024, QualityPreset::Low),
            (6 * 1024 - 1, QualityPreset::Low),
            (6 * 1024, QualityPreset::Medium),
            (8 * 1024 - 1, QualityPreset::Medium),
            (8 * 1024, QualityPreset::High),
            (24 * 1024, QualityPreset::High),
        ] {
            for ray_tracing in [false, true] {
                let caps = Capabilities {
                    ray_tracing,
                    device_local_mib: Some(mib),
                };
                assert_eq!(QualityPreset::detect(&caps), expected);
                let plan = Plan::resolve(
                    QualityChoice::Auto,
                    &caps,
                    [2560, 1440],
                    RenderOptions::default(),
                )
                .unwrap();
                assert_eq!(plan.preset, expected);
            }
        }
    }

    #[test]
    fn raster_scale_keeps_the_1440p_pixel_budget_on_smaller_displays() {
        for preset in QualityPreset::ALL {
            let scale = preset.raster_scale();
            // At and above the budget display: the preset scale, unchanged.
            assert_eq!(preset.raster_scale_for(BUDGET_DISPLAY), scale);
            assert_eq!(preset.raster_scale_for([3840, 2160]), scale);
            // Below it: the same pixel count, never above native.
            let hd = preset.raster_scale_for([1920, 1080]);
            let pixels = |s: f32, d: [u32; 2]| s * s * d[0] as f32 * d[1] as f32;
            assert!(hd > scale && hd <= 1.);
            assert!((pixels(hd, [1920, 1080]) - pixels(scale, BUDGET_DISPLAY)).abs() < 2e3);
            assert_eq!(preset.raster_scale_for([1131, 606]), 1., "{preset:?}");
        }
        assert!((QualityPreset::High.raster_scale_for([1920, 1080]) - 0.8).abs() < 1e-4);
    }
    #[test]
    fn raster_by_default_and_ray_tracing_is_opt_in() {
        let traced = RenderOptions { ray_tracing: true };
        let raster = Capabilities {
            ray_tracing: false,
            device_local_mib: None,
        };
        let display = [2560, 1440];
        for caps in [RT, raster] {
            for preset in QualityPreset::ALL {
                let choice = QualityChoice::Preset(preset);
                let plan = Plan::resolve(choice, &caps, display, RenderOptions::default()).unwrap();
                assert_eq!(plan.preset, preset);
                let scale = preset.raster_scale();
                let render = display.map(|v| (v as f32 * scale).round() as u32);
                assert!(plan.raster.render_size == render && plan.raster.display_size == display);
                assert!(!plan.rt_fallback);
            }
        }
        // Non-RT devices keep the requested preset on raster even when opting in,
        // and report the declined opt-in (ARA6008).
        let choice = QualityChoice::Preset(QualityPreset::Epic);
        let epic = Plan::resolve(choice, &raster, display, traced).unwrap();
        assert_eq!(epic.preset, QualityPreset::Epic);
        assert!(epic.rt_fallback);
        for preset in QualityPreset::ALL {
            let plan = Plan::resolve(QualityChoice::Preset(preset), &RT, display, traced).unwrap();
            assert!(!plan.rt_fallback);
        }
        let auto = Plan::resolve(
            QualityChoice::Auto,
            &raster,
            display,
            RenderOptions::default(),
        );
        assert_eq!(auto.unwrap().preset, QualityPreset::Low);
    }

    #[test]
    fn rtx_4070_super_class_auto_is_high_on_a_raster_only_device() {
        let caps = Capabilities {
            ray_tracing: false,
            device_local_mib: Some(12 * 1024),
        };
        assert_eq!(
            QualityPreset::detect(&caps),
            QualityPreset::High,
            "editor and runtime must agree"
        );
        let plan = Plan::resolve(
            QualityChoice::Auto,
            &caps,
            [2560, 1440],
            RenderOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.preset, QualityPreset::High);
    }

    #[test]
    fn ray_queries_never_run_without_the_opt_in() {
        let raster = Capabilities {
            ray_tracing: false,
            ..RT
        };
        for preset in QualityPreset::ALL {
            let choice = QualityChoice::Preset(preset);
            let hybrid = |caps| {
                let plan = Plan::resolve(choice, caps, [2560, 1440], Default::default())
                    .unwrap()
                    .raster;
                plan.quality.validate().unwrap();
                plan.hybrid()
            };
            assert!(!hybrid(&RT), "{preset}");
            assert!(!hybrid(&raster), "{preset}");
        }
    }

    #[test]
    fn choices_parse_and_print() {
        for text in ["auto", "low", "Medium", "HIGH", "epic"] {
            let choice: QualityChoice = text.parse().unwrap();
            assert_eq!(choice.to_string(), text.to_ascii_lowercase());
        }
        assert!("ultra".parse::<QualityChoice>().is_err());
    }
}
