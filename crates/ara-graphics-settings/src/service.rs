use crate::{Capabilities, INVALID, Preset, Settings, Store};
use ara_diag::Diagnostic;
use bevy_ecs::prelude::Resource;
use serde::{Deserialize, Serialize};

/// Shared script/protocol operations. Only Set/Reset/Detect change the candidate;
/// Apply/Revert publish it and Save persists the applied preferences explicitly.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    /// Read pending/applied settings and hardware/display capabilities.
    Get,
    /// Replace the complete pending candidate.
    Set {
        /// Candidate to validate on Apply.
        settings: Settings,
    },
    /// Validate and publish pending settings at the next presentation boundary.
    Apply,
    /// Apply the previous configuration (including a just-applied display mode).
    Revert,
    /// Stage the project's defaults; Apply and Save remain explicit.
    ResetToDefaults,
    /// Persist applied preferences, including next-start ray tracing.
    Save,
    /// Stage device-based Auto and clear group/scale/upscaler overrides.
    AutoDetect,
}

/// Immutable service snapshot returned by scripts and the engine protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Editable candidate; never persisted until applied.
    pub pending: Settings,
    /// Last accepted preferences; renderer adopts these at its next frame boundary.
    pub applied: Settings,
    /// Project defaults used by ResetToDefaults.
    pub defaults: Settings,
    /// Current device and display capabilities.
    pub capabilities: Capabilities,
    /// Monotonic accepted application number, scoped to this player.
    pub revision: u64,
    /// Ray-tracing preference differs from the device opened at startup.
    pub restart_required: bool,
    /// Current device's immutable ray-tracing opt-in.
    pub active_ray_tracing: bool,
    /// Validation and persistence diagnostics for the last operation.
    pub diagnostics: Vec<Diagnostic>,
}

/// Host-owned presentation resource, deliberately absent from gameplay snapshots.
#[derive(Resource)]
pub struct Service {
    state: State,
    previous: Settings,
    store: Option<Store>,
}

impl Default for Service {
    fn default() -> Self {
        Self::new(Settings::default(), None)
    }
}

impl Service {
    /// Load a user file over project defaults, without saving or enabling devices.
    pub fn new(mut defaults: Settings, mut store: Option<Store>) -> Self {
        let mut diagnostics = defaults.validate();
        let settings = match &mut store {
            Some(store) => {
                let (settings, rows) = store.load(&defaults);
                diagnostics.extend(rows);
                settings
            }
            None => defaults.clone(),
        };
        let active_ray_tracing = settings.ray_tracing;
        Self {
            previous: settings.clone(),
            store,
            state: State {
                pending: settings.clone(),
                applied: settings,
                defaults,
                capabilities: Capabilities::default(),
                revision: 1,
                restart_required: false,
                active_ray_tracing,
                diagnostics,
            },
        }
    }

    /// Borrow preferences without copying or allocating in the frame loop.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Publish physical device/display capabilities. Never infer physical RT support
    /// from whether ray queries were enabled on the logical device.
    pub fn set_capabilities(&mut self, caps: Capabilities) {
        self.state.capabilities = caps;
    }

    /// Startup command-line overrides win over saved preferences for this launch.
    /// Call before opening any GPU device or running scripts.
    pub fn startup(&mut self, mut settings: Settings) {
        self.state.diagnostics.extend(settings.validate());
        self.state.active_ray_tracing = settings.ray_tracing;
        self.state.applied = settings.clone();
        self.state.pending = settings.clone();
        self.previous = settings;
    }

    /// Execute one typed settings operation. Invalid capabilities reject Apply
    /// atomically, retaining the candidate and the previous applied configuration.
    pub fn request(&mut self, request: Request) -> Result<State, Diagnostic> {
        self.state.diagnostics.clear();
        match request {
            Request::Get => {}
            Request::Set { settings } => self.state.pending = settings,
            Request::ResetToDefaults => self.state.pending = self.state.defaults.clone(),
            Request::AutoDetect => {
                self.state.pending.preset = Preset::Auto;
                self.state.pending.groups = Default::default();
                self.state.pending.resolution_scale = None;
                self.state.pending.upscaler = Default::default();
            }
            Request::Apply | Request::Revert => {
                let mut next = if matches!(request, Request::Revert) {
                    self.previous.clone()
                } else {
                    self.state.pending.clone()
                };
                let rows = next.validate();
                let caps = &self.state.capabilities;
                if next.hdr && !caps.hdr_supported {
                    return Err(Diagnostic::error(
                        INVALID,
                        "HDR10 output is unavailable on this player surface",
                    ));
                }
                if next.ray_tracing && caps.device_known && !caps.ray_tracing_supported {
                    return Err(Diagnostic::error(
                        INVALID,
                        "hardware ray tracing is unsupported on this GPU",
                    ));
                }
                if next.monitor != 0
                    && caps.display_available
                    && !caps.modes.iter().any(|m| m.monitor == next.monitor)
                {
                    return Err(Diagnostic::error(
                        INVALID,
                        "the selected monitor is no longer connected",
                    ));
                }
                self.state.diagnostics = rows;
                self.previous = std::mem::replace(&mut self.state.applied, next.clone());
                self.state.pending = next;
                self.state.restart_required =
                    self.state.applied.ray_tracing != self.state.active_ray_tracing;
                self.state.revision += 1;
            }
            Request::Save => {
                self.store
                    .as_ref()
                    .ok_or_else(|| {
                        Diagnostic::error(crate::STORAGE, "this host has no user settings store")
                    })?
                    .save(&self.state.applied)?;
            }
        }
        Ok(self.state.clone())
    }
}
