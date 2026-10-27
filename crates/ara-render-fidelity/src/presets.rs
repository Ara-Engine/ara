//! Quality presets: one user-facing choice mapped onto renderer selection, render
//! resolution and work budgets. Hosts resolve a [`Plan`] once per view (and again on
//! resize or device change); every plan keeps temporal antialiasing on.
//!
//! D27: every preset is raster by default ([`RasterPlan`], `ara_render::look::Quality`);
//! the ray-traced fidelity renderer is an opt-in ([`RenderOptions::ray_tracing`]).
use crate::{FidelityView, Mode, PostSettings, Settings, Tier, Upscaler};
use anyhow::{Result, bail};
use ara_gpu::GpuDevice;
use ara_render::look::{
    AmbientOcclusion, DepthOfFieldQuality, FarShadows, LocalShadows, MotionBlurQuality, Quality,
    RayQueries, ReflectionCaptures, Reflections, Shadows, VirtualShadows,
};

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
    /// Raster: four 2048 cascades to 150 m; on RT devices (D29) hardware ray
    /// queries reflect surfaces below roughness 0.3. Ray traced: ReSTIR at half
    /// scale, paths ending in the probe field after one traced event.
    High,
    /// Raster: four cascades to 250 m, the largest GI budget; on RT devices ray
    /// queries reflect surfaces below roughness 0.5. Ray traced: ReSTIR at
    /// two-thirds scale with two traced events before the probe field.
    Epic,
}

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

impl Capabilities {
    /// Reads the capabilities of an opened device.
    pub fn of(gpu: &GpuDevice) -> Self {
        Self {
            ray_tracing: gpu.info().ray_tracing,
            device_local_mib: gpu.info().quality_memory_mib(),
        }
    }
}

/// When ReSTIR direct/indirect reservoirs run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservoirs {
    /// Never; lights use streaming RIS and diffuse paths end in DDGI.
    Off,
    /// Only for scenes with emissive triangles or many local lights.
    ManyLights,
    /// Always.
    On,
}

/// Fidelity work budgets selected by a preset. Apply with [`FidelityPlan::settings`]
/// at view creation and [`FidelityPlan::configure`] after `new` or `resize`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FidelityPlan {
    /// Transport resolution.
    pub render_size: [u32; 2],
    /// Display resolution.
    pub display_size: [u32; 2],
    /// Temporal reconstruction (always TAAU: the spatial FSR1 path has no history).
    pub upscaler: Upscaler,
    /// DDGI probe paths per frame.
    pub probe_ray_budget: u32,
    /// Roughness above which reflections use only the probe field.
    pub reflection_cutoff: f32,
    /// Traced secondary surfaces before the DDGI terminal.
    pub gather_events: u32,
    /// Pixels per incident sample per frame (2 or 4).
    pub transport_stride: u32,
    /// ReSTIR policy.
    pub reservoirs: Reservoirs,
    /// Bounded reservoir budgets: paths end in the DDGI terminal after
    /// `gather_events` and glossy specular traces at half rate (see
    /// [`FidelityView::set_bounded_reservoirs`]); Epic keeps full budgets.
    pub bounded_reservoirs: bool,
}

impl FidelityPlan {
    /// View settings for [`FidelityView::new`] with the scene's post settings.
    pub fn settings(&self, post: PostSettings) -> Settings {
        Settings {
            render_size: self.render_size,
            display_size: self.display_size,
            tier: Tier::Rt,
            mode: Mode::Hybrid,
            upscaler: self.upscaler,
            probe_ray_budget: self.probe_ray_budget,
            reflection_cutoff: self.reflection_cutoff,
            post,
        }
    }

    /// Applies the per-view budgets at a frame boundary. ReSTIR under `ManyLights`
    /// follows the view's current scene, so call again after replacing content.
    pub fn configure(&self, gpu: &mut GpuDevice, view: &mut FidelityView) -> Result<()> {
        view.set_lighting_budget(self.probe_ray_budget, self.reflection_cutoff)?;
        view.set_gather_events(self.gather_events)?;
        view.set_transport_stride(self.transport_stride)?;
        view.set_bounded_reservoirs(self.bounded_reservoirs);
        let reservoirs = match self.reservoirs {
            Reservoirs::Off => false,
            Reservoirs::On => true,
            Reservoirs::ManyLights => view.scene.has_emitters || view.scene.many_local_lights,
        };
        view.set_reservoir_sampling(gpu, reservoirs)
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
    /// Shadows, probe GI budget and post effects. D29: on devices with hardware ray
    /// queries High and Epic keep `quality.ray_queries` (the hybrid path); other
    /// devices resolve the same preset without it (the raster High/Epic path).
    pub quality: Quality,
}

impl RasterPlan {
    /// Whether this plan adds hardware ray queries to the raster renderer (D29's
    /// hybrid High/Epic path on RT devices).
    pub fn hybrid(&self) -> bool {
        self.quality.ray_queries.is_some()
    }
}

/// Renderer selection options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderOptions {
    /// Opt in to the ray-traced fidelity renderer for Medium and above on devices
    /// with hardware ray queries (D27: raster is the default on every GPU).
    pub ray_tracing: bool,
}

/// Which renderer draws a view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderPath {
    /// The raster renderer (`ara-render`) with the look, probe GI and TAA.
    Raster(RasterPlan),
    /// The ray-traced fidelity renderer (opt-in).
    Fidelity(FidelityPlan),
}

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
            Self::High => 2. / 3.,
            Self::Epic => 0.77,
        }
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
        // the roughness cut. `Plan::resolve` drops them on devices without RT.
        let rays = |reflection_roughness, distance| {
            Some(RayQueries {
                reflection_roughness,
                distance,
            })
        };
        match self {
            // LOD error and the cull size are in render pixels (about two
            // display pixels each at Low's scale).
            Self::Low => Quality {
                shadows: shadows(2, 512, 60., (1024, 600.)),
                ambient_occlusion: None,
                reflections: None,
                gi_budget: 256,
                bloom: false,
                emitter_lights: 4,
                lod_pixels: 6.,
                min_pixels: 6.,
                high_tier: false,
                reflection_captures: None,
                terrain_detail: 0.,
                local_shadows: None,
                horizon: Some(1024),
                virtual_shadows: None,
                depth_of_field: dof(12, 0.01),
                motion_blur: None,
                ray_queries: None,
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
            },
            Self::High => Quality {
                shadows: shadows(4, 2048, 150., (2048, 1500.)),
                ambient_occlusion: ao(2, 6),
                reflections: ssr(0.5, 30., 24),
                gi_budget: 2048,
                bloom: true,
                emitter_lights: 8,
                lod_pixels: 1.,
                min_pixels: 1.5,
                high_tier: true,
                reflection_captures: Some(ReflectionCaptures {
                    resolution: 512,
                    captures: 8,
                }),
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
                ray_queries: rays(0.3, 1500.),
            },
            Self::Epic => Quality {
                shadows: shadows(4, 2048, 250., (2048, 2000.)),
                ambient_occlusion: ao(3, 8),
                reflections: ssr(0.6, 50., 32),
                gi_budget: 4096,
                bloom: true,
                emitter_lights: 16,
                lod_pixels: 1.,
                min_pixels: 1.,
                high_tier: true,
                reflection_captures: Some(ReflectionCaptures {
                    resolution: 512,
                    captures: 16,
                }),
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
                ray_queries: rays(0.5, 2000.),
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
    /// Renderer and budgets.
    pub path: RenderPath,
}

impl Plan {
    /// Resolves a choice for a device and display size. Every preset is raster
    /// unless `options` opts in to ray tracing on a capable device; then Medium and
    /// above use the fidelity renderer (Low stays raster). Raster runs every preset
    /// on every device, so nothing is downgraded.
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
        // D29: High/Epic add hardware ray queries only where the device has them.
        let quality = Quality {
            ray_queries: preset
                .raster_quality()
                .ray_queries
                .filter(|_| capabilities.ray_tracing),
            ..preset.raster_quality()
        };
        let raster = RenderPath::Raster(RasterPlan {
            render_size: scaled(preset.raster_scale()),
            display_size,
            quality,
        });
        let path = match preset {
            _ if !traced => raster,
            QualityPreset::Low => raster,
            QualityPreset::Medium => RenderPath::Fidelity(FidelityPlan {
                render_size: scaled(0.4),
                display_size,
                upscaler: Upscaler::Taau,
                probe_ray_budget: 8192,
                reflection_cutoff: 0.35,
                gather_events: 1,
                transport_stride: 4,
                reservoirs: Reservoirs::On,
                bounded_reservoirs: true,
            }),
            QualityPreset::High => RenderPath::Fidelity(FidelityPlan {
                render_size: scaled(0.5),
                display_size,
                upscaler: Upscaler::Taau,
                probe_ray_budget: 16384,
                reflection_cutoff: 0.6,
                gather_events: 1,
                transport_stride: 4,
                reservoirs: Reservoirs::On,
                bounded_reservoirs: true,
            }),
            QualityPreset::Epic => RenderPath::Fidelity(FidelityPlan {
                render_size: scaled(2.0 / 3.0),
                display_size,
                upscaler: Upscaler::Taau,
                probe_ray_budget: 32768,
                reflection_cutoff: 0.8,
                gather_events: 2,
                transport_stride: 4,
                reservoirs: Reservoirs::On,
                bounded_reservoirs: true,
            }),
        };
        Ok(Self {
            preset,
            requested,
            path,
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
                assert!(matches!(plan.path, RenderPath::Raster(_)));
            }
        }
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
                assert!(matches!(plan.path,
                    RenderPath::Raster(r) if r.render_size == render && r.display_size == display));
            }
        }
        // Non-RT devices keep the requested preset on raster even when opting in.
        let choice = QualityChoice::Preset(QualityPreset::Epic);
        let epic = Plan::resolve(choice, &raster, display, traced).unwrap();
        assert_eq!(epic.preset, QualityPreset::Epic);
        assert!(matches!(epic.path, RenderPath::Raster(_)));
        for preset in QualityPreset::ALL {
            let plan = Plan::resolve(QualityChoice::Preset(preset), &RT, display, traced).unwrap();
            match plan.path {
                RenderPath::Fidelity(f) => {
                    assert_ne!(preset, QualityPreset::Low);
                    assert_eq!(f.upscaler, Upscaler::Taau);
                    f.settings(PostSettings::default()).validate().unwrap();
                }
                RenderPath::Raster(_) => assert_eq!(preset, QualityPreset::Low),
            }
        }
        let high = Plan::resolve(QualityChoice::Auto, &RT, display, traced).unwrap();
        assert!(matches!(high.path, RenderPath::Fidelity(f) if f.render_size == [1280, 720]));
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
        assert!(matches!(plan.path, RenderPath::Raster(_)));
    }

    #[test]
    fn high_and_epic_add_ray_queries_only_on_rt_devices() {
        let raster = Capabilities {
            ray_tracing: false,
            ..RT
        };
        for preset in QualityPreset::ALL {
            let choice = QualityChoice::Preset(preset);
            let hybrid = |caps| match Plan::resolve(choice, caps, [2560, 1440], Default::default())
                .unwrap()
                .path
            {
                RenderPath::Raster(plan) => {
                    plan.quality.validate().unwrap();
                    plan.hybrid()
                }
                RenderPath::Fidelity(_) => unreachable!("raster by default"),
            };
            assert_eq!(hybrid(&RT), preset >= QualityPreset::High, "{preset}");
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
