//! Quality presets: one user-facing choice mapped onto renderer selection, transport
//! resolution and fidelity work budgets. Hosts resolve a [`Plan`] once per view (and
//! again on resize or device change); every plan keeps temporal antialiasing on.
use crate::{FidelityView, Mode, PostSettings, Settings, Tier, Upscaler};
use anyhow::{Result, bail};
use ara_gpu::vulkan::VulkanDevice;

/// User-facing quality tier, from the cheapest baseline raster to native-resolution
/// ray-traced many-light sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualityPreset {
    /// Baseline raster renderer with live TAA; no ray tracing.
    Low,
    /// Hybrid ray-traced lighting at half resolution: DDGI with one traced event,
    /// probe-only rough reflections, no ReSTIR.
    Medium,
    /// Hybrid ray-traced lighting at 0.625 scale (1600x900 for 1440p); ReSTIR in
    /// scenes lit by emissive surfaces or many local lights, with reservoir paths
    /// ending in the probe field after one traced event.
    High,
    /// Native-resolution ray tracing with ReSTIR DI/GI in every scene.
    Epic,
}

impl QualityPreset {
    /// Every preset, cheapest first.
    pub const ALL: [QualityPreset; 4] = [Self::Low, Self::Medium, Self::High, Self::Epic];

    /// The default preset for a device: Epic is opt-in, RT devices start at High
    /// (Medium with under 8 GiB of device-local memory), and others use Low.
    pub fn detect(capabilities: &Capabilities) -> Self {
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
    /// Device-local memory in MiB, when the device reports it.
    pub device_local_mib: Option<u64>,
}

impl Capabilities {
    /// Reads the capabilities of an opened device.
    pub fn of(gpu: &VulkanDevice) -> Self {
        Self {
            ray_tracing: gpu.context().info.ray_tracing,
            device_local_mib: None,
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
    /// Reservoir secondary paths end in the DDGI terminal after `gather_events`
    /// (see [`FidelityView::set_bounded_reservoir_paths`]); Epic traces full paths.
    pub bounded_reservoir_paths: bool,
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
    pub fn configure(&self, gpu: &mut VulkanDevice, view: &mut FidelityView) -> Result<()> {
        view.set_lighting_budget(self.probe_ray_budget, self.reflection_cutoff)?;
        view.set_gather_events(self.gather_events)?;
        view.set_transport_stride(self.transport_stride)?;
        view.set_bounded_reservoir_paths(self.bounded_reservoir_paths);
        let reservoirs = match self.reservoirs {
            Reservoirs::Off => false,
            Reservoirs::On => true,
            Reservoirs::ManyLights => view.scene.has_emitters || view.scene.many_local_lights,
        };
        view.set_reservoir_sampling(gpu, reservoirs)
    }
}

/// Which renderer draws a view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderPath {
    /// The baseline raster renderer (`ara-render`) in live mode, with TAA.
    Baseline {
        /// Render and display resolution (the baseline does not upscale).
        render_size: [u32; 2],
    },
    /// The fidelity renderer.
    Fidelity(FidelityPlan),
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
    /// Resolves a choice for a device and display size. Presets above Low need
    /// hardware ray queries (the raster tier's software visibility does not scale
    /// to real content), so non-RT devices run Low whatever was requested.
    pub fn resolve(
        choice: QualityChoice,
        capabilities: &Capabilities,
        display_size: [u32; 2],
    ) -> Result<Self> {
        if display_size.contains(&0) || display_size.iter().any(|v| *v > 8192) {
            bail!("Invalid display size {display_size:?}");
        }
        let requested = match choice {
            QualityChoice::Auto => QualityPreset::detect(capabilities),
            QualityChoice::Preset(p) => p,
        };
        let preset = if capabilities.ray_tracing {
            requested
        } else {
            QualityPreset::Low
        };
        let scaled = |scale: f32| display_size.map(|v| ((v as f32 * scale).round() as u32).max(1));
        let path = match preset {
            QualityPreset::Low => RenderPath::Baseline {
                render_size: display_size,
            },
            QualityPreset::Medium => RenderPath::Fidelity(FidelityPlan {
                render_size: scaled(0.5),
                display_size,
                upscaler: Upscaler::Taau,
                probe_ray_budget: 8192,
                reflection_cutoff: 0.35,
                gather_events: 1,
                transport_stride: 4,
                reservoirs: Reservoirs::Off,
                bounded_reservoir_paths: true,
            }),
            QualityPreset::High => RenderPath::Fidelity(FidelityPlan {
                render_size: scaled(0.625),
                display_size,
                upscaler: Upscaler::Taau,
                probe_ray_budget: 16384,
                reflection_cutoff: 0.6,
                gather_events: 1,
                transport_stride: 4,
                reservoirs: Reservoirs::ManyLights,
                bounded_reservoir_paths: true,
            }),
            QualityPreset::Epic => RenderPath::Fidelity(FidelityPlan {
                render_size: display_size,
                display_size,
                upscaler: Upscaler::Taau,
                probe_ray_budget: 32768,
                reflection_cutoff: 0.8,
                gather_events: 2,
                transport_stride: 2,
                reservoirs: Reservoirs::On,
                bounded_reservoir_paths: false,
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
    }

    #[test]
    fn non_rt_devices_run_low_and_every_plan_is_temporal() {
        let raster = Capabilities {
            ray_tracing: false,
            device_local_mib: None,
        };
        let plan = Plan::resolve(
            QualityChoice::Preset(QualityPreset::Epic),
            &raster,
            [2560, 1440],
        )
        .unwrap();
        assert_eq!(
            (plan.requested, plan.preset),
            (QualityPreset::Epic, QualityPreset::Low)
        );
        for preset in QualityPreset::ALL {
            let plan = Plan::resolve(QualityChoice::Preset(preset), &RT, [2560, 1440]).unwrap();
            assert_eq!(plan.preset, preset);
            if let RenderPath::Fidelity(f) = plan.path {
                assert_eq!(f.upscaler, Upscaler::Taau);
                f.settings(PostSettings::default()).validate().unwrap();
            }
        }
        let high = Plan::resolve(QualityChoice::Auto, &RT, [2560, 1440]).unwrap();
        assert!(matches!(high.path, RenderPath::Fidelity(f) if f.render_size == [1600, 900]));
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
