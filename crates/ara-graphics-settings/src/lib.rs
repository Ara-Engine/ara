#![warn(missing_docs)]
//! User graphics preferences: typed candidates, explicit apply/revert/save and safe storage.
mod model;
mod service;
mod store;
pub use model::*;
pub use service::{Request, Service, State};
pub use store::Store;

use ara_diag::{Code, Diagnostic};

/// Invalid settings or a requested capability which this player cannot provide.
pub const INVALID: Code = Code(3500);
/// A settings file could not be read, migrated or atomically saved.
pub const STORAGE: Code = Code(3501);

impl Settings {
    /// Clamp bounded numeric preferences. Nonfinite scale reverts to the preset.
    /// Returns a diagnostic for each adjusted field; never enables ray tracing.
    pub fn validate(&mut self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        macro_rules! clamp {
            ($field:ident, $min:expr, $max:expr) => {
                let value = self.$field.clamp($min, $max);
                if value != self.$field {
                    self.$field = value;
                    diagnostics.push(Diagnostic::warning(
                        INVALID,
                        concat!(stringify!($field), " was clamped to its supported range"),
                    ));
                }
            };
        }
        clamp!(width, 320, 8192);
        clamp!(height, 180, 8192);
        if self.frame_rate_cap != 0 {
            clamp!(frame_rate_cap, 15, 1000);
        }
        if self.refresh_rate_millihz != 0 {
            clamp!(refresh_rate_millihz, 15_000, 1_000_000);
        }
        if let Some(scale) = self.resolution_scale {
            let valid = scale.is_finite().then(|| scale.clamp(0.1, 1.));
            if valid != self.resolution_scale {
                self.resolution_scale = valid;
                diagnostics.push(Diagnostic::warning(
                    INVALID,
                    "resolutionScale was normalized to 0.1..1 or the preset",
                ));
            }
        }
        if let Some(budget) = &mut self.texture_budget_mib {
            let valid = (*budget).clamp(128, 32768);
            if valid != *budget {
                *budget = valid;
                diagnostics.push(Diagnostic::warning(
                    INVALID,
                    "textureBudgetMib was clamped to 128..32768",
                ));
            }
        }
        diagnostics
    }

    /// Resolve Auto using the same memory thresholds as renderer preset detection.
    pub fn tier(&self, caps: &Capabilities) -> Tier {
        match self.preset {
            Preset::Low => Tier::Low,
            Preset::Medium => Tier::Medium,
            Preset::High => Tier::High,
            Preset::Epic => Tier::Epic,
            Preset::Auto => match caps.device_local_mib {
                Some(mib) if mib >= 8192 => Tier::High,
                Some(mib) if mib >= 6144 => Tier::Medium,
                _ => Tier::Low,
            },
        }
    }

    /// Texture residency cap. Leave at least half of known VRAM for other GPU work.
    pub fn texture_mib(&self, caps: &Capabilities) -> u32 {
        let tier = self.groups.textures.unwrap_or(self.tier(caps));
        let requested = self
            .texture_budget_mib
            .unwrap_or([256, 512, 1024, 2048][tier.index()]);
        requested.min(
            caps.device_local_mib
                .map_or(32768, |mib| (mib / 2).clamp(128, 32768) as u32),
        )
    }
}
