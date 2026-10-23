//! The per-update acoustics pass: from ECS sources, the listener and scene geometry to
//! voice parameters, the listener's room reverb and an occlusion trace for agents.

use crate::{
    bake::ProbeField,
    geometry::RayScene,
    material::{AcousticMaterial, MaterialPreset},
    math::{add, normalize_or, scale, sub},
    occlusion::{Occlusion, OcclusionQuery, OcclusionSolver},
    propagation::{AcousticSource, AcousticsSettings, curve_gain},
    reverb::ReverbParams,
    room::{AcousticPortal, ReverbZone, ZoneSet},
};
use ara_audio::{
    AudioError, AudioListener, AudioSource, Result, VoiceParameters,
    capture::{Blocker, OcclusionRecord},
    diagnose::OCCLUDED_BELOW,
    spatial::{AcousticPath, evaluate},
};

/// How far around the listener baked probes are searched, in meters.
pub const PROBE_RADIUS: f32 = 30.0;

/// One emitter to evaluate.
#[derive(Debug, Clone, Copy)]
pub struct Emitter<'a> {
    /// Entity label for traces and diagnostics (`e_8b21`).
    pub label: &'a str,
    /// The source component, with position/velocity from the transform.
    pub source: &'a AudioSource,
    /// Acoustic behaviour; use `AcousticSource::default()` when absent.
    pub acoustic: &'a AcousticSource,
}

/// Result for one emitter.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// Ready-to-send voice parameters (`VoiceCommand::Set`).
    pub parameters: VoiceParameters,
    /// Agent-facing trace (attach to `capture::Analysis::occlusion`).
    pub record: OcclusionRecord,
    /// Straight-line source-listener distance.
    pub distance: f32,
    /// Distance/cone attenuation applied, excluding source gain.
    pub attenuation: f32,
}

/// Acoustics state for one world: settings, zones, resident baked cells and buffers.
/// Run `evaluate` on an audio-control worker at the presentation rate, never per sample.
pub struct Acoustics {
    settings: AcousticsSettings,
    zones: ZoneSet,
    probes: ProbeField,
    solver: OcclusionSolver,
    queries: Vec<OcclusionQuery>,
    owners: Vec<usize>,
    results: Vec<Occlusion>,
}

fn material_name(m: &AcousticMaterial) -> &'static str {
    MaterialPreset::ALL
        .into_iter()
        .find(|p| p.material() == *m)
        .map_or("custom", MaterialPreset::name)
}

impl Acoustics {
    /// A world without zones or baked cells.
    pub fn new(settings: AcousticsSettings) -> Result<Self> {
        Ok(Self {
            zones: ZoneSet::new(&[], &[], settings)?,
            settings,
            probes: ProbeField::new(),
            solver: OcclusionSolver::new(),
            queries: Vec::new(),
            owners: Vec::new(),
            results: Vec::new(),
        })
    }

    /// Replaces zones and portals (rebuild when their components change).
    pub fn set_zones(&mut self, zones: &[ReverbZone], portals: &[AcousticPortal]) -> Result<()> {
        self.zones = ZoneSet::new(zones, portals, self.settings)?;
        Ok(())
    }
    /// The validated zones.
    pub fn zones(&self) -> &ZoneSet {
        &self.zones
    }
    /// Resident baked cells; load/unload with world streaming.
    pub fn probes_mut(&mut self) -> &mut ProbeField {
        &mut self.probes
    }
    /// Propagation constants.
    pub fn settings(&self) -> &AcousticsSettings {
        &self.settings
    }

    fn zone_params(&self, zone: usize, listener: &AudioListener) -> ReverbParams {
        let baked = self
            .zones
            .zone(zone)
            .use_baked
            .then(|| {
                self.probes.sample(listener.position, PROBE_RADIUS, |p| {
                    self.zones.zone_at(p) == Some(zone)
                })
            })
            .flatten();
        self.zones.params(zone, listener, baked.map(|s| s.rt60))
    }

    /// The listener's room reverb: its zone (crossfaded with the outer space near the
    /// boundary), baked probe times inside zones, or baked probes alone outside zones.
    pub fn room(&self, listener: &AudioListener) -> ReverbParams {
        let place = self.zones.listener(listener.position);
        let outer = match place.outer {
            Some(z) => self.zone_params(z, listener),
            None => self.outdoors(listener),
        };
        match place.zone {
            Some(z) => self
                .zone_params(z, listener)
                .blend(&outer, 1.0 - place.weight),
            None => outer,
        }
    }

    fn outdoors(&self, listener: &AudioListener) -> ReverbParams {
        let Some(sample) = self.probes.sample(listener.position, PROBE_RADIUS, |p| {
            self.zones.zone_at(p).is_none()
        }) else {
            return ReverbParams::silent();
        };
        ReverbParams {
            rt60: sample.rt60,
            mean_free_path: sample.mean_free_path,
            level: sample.level,
            wet: 1.0,
            room: 1,
            ..ReverbParams::silent()
        }
    }

    fn attenuation(
        source: &AudioSource,
        acoustic: &AcousticSource,
        distance: f32,
        cone: f32,
        model: f32,
    ) -> f32 {
        if !source.spatial {
            return 1.0;
        }
        match curve_gain(&acoustic.distance_curve, distance) {
            Some(gain) => gain * cone,
            None => model,
        }
    }

    /// Evaluates every emitter with one batched ray trace. Results follow emitter order.
    pub fn evaluate(
        &mut self,
        scene: &mut dyn RayScene,
        listener: &AudioListener,
        emitters: &[Emitter<'_>],
        out: &mut Vec<Evaluation>,
    ) -> Result<()> {
        self.queries.clear();
        self.owners.clear();
        for (i, e) in emitters.iter().enumerate() {
            if e.acoustic.occlusion && e.source.spatial {
                self.queries.push(OcclusionQuery {
                    source: e.source.position,
                    listener: listener.position,
                    radius: e.acoustic.occlusion_radius,
                    rays: self.settings.occlusion_rays,
                });
                self.owners.push(i);
            }
        }
        self.solver
            .solve_batch(scene, &self.queries, &mut self.results)?;
        out.clear();
        let listener_zone = self.zones.zone_at(listener.position);
        let mut next_result = 0;
        for (i, e) in emitters.iter().enumerate() {
            let occlusion = if self.owners.get(next_result) == Some(&i) {
                next_result += 1;
                self.results[next_result - 1].clone()
            } else {
                Occlusion::clear()
            };
            out.push(self.emitter(scene, listener, listener_zone, e, &occlusion)?);
        }
        Ok(())
    }

    fn emitter(
        &self,
        scene: &dyn RayScene,
        listener: &AudioListener,
        listener_zone: Option<usize>,
        e: &Emitter<'_>,
        occlusion: &Occlusion,
    ) -> Result<Evaluation> {
        let (source, acoustic) = (e.source, e.acoustic);
        if !acoustic.reverb_send.is_finite() || !acoustic.doppler.is_finite() {
            return Err(AudioError::Invalid("acoustic source values"));
        }
        let spatial = evaluate(source, listener)?;
        let straight = spatial.distance;
        let direct_gain = Self::attenuation(
            source,
            acoustic,
            straight,
            spatial.cone,
            spatial.attenuation,
        );
        let v = occlusion.visibility;
        let direct: [f32; 3] = std::array::from_fn(|b| v + (1.0 - v) * occlusion.transmission[b]);
        let source_zone = self.zones.zone_at(source.position);
        let route = if source.spatial && !self.zones.is_empty() {
            self.zones.route(source.position, listener.position)
        } else {
            None
        };
        let mut bands = direct;
        let mut direction = spatial.direction;
        let mut gain = direct_gain;
        let mut path_length = straight;
        let mut portals = Vec::new();
        if let Some(route) = &route {
            // Re-evaluate distance and direction as if the source sat at the last portal,
            // pushed back to the full path length.
            let toward = normalize_or(sub(route.apparent, listener.position), [0.0, 0.0, -1.0]);
            let mut moved = source.clone();
            moved.position = add(listener.position, scale(toward, route.length));
            let via_spatial = evaluate(&moved, listener)?;
            let via_gain = Self::attenuation(
                source,
                acoustic,
                route.length,
                via_spatial.cone,
                via_spatial.attenuation,
            );
            let ratio = if direct_gain > 0.0 {
                via_gain / direct_gain
            } else {
                0.0
            };
            let via: [f32; 3] = std::array::from_fn(|b| route.gains[b] * ratio);
            bands = std::array::from_fn(|b| (direct[b].powi(2) + via[b].powi(2)).sqrt().min(1.0));
            if via[1] > direct[1] {
                direction = via_spatial.direction;
                if direct_gain == 0.0 {
                    gain = via_gain;
                    bands = route.gains;
                }
                path_length = route.length;
            }
            portals = route
                .portals
                .iter()
                .map(|&p| {
                    let portal = self.zones.portal(p);
                    format!(
                        "{}<->{}",
                        self.zones.name(self.zone_index(&portal.a)),
                        self.zones.name(self.zone_index(&portal.b))
                    )
                })
                .collect();
        }
        let transmission = if v < 1.0 {
            bands.map(|b| ((b - v) / (1.0 - v)).clamp(0.0, 1.0))
        } else {
            [0.0; 3]
        };
        let air = if acoustic.air_absorption && source.spatial {
            self.settings.air(path_length)
        } else {
            [1.0; 3]
        };
        // Diffuse reverberant level is distance independent: undo the voice's distance
        // attenuation in the send (bounded at +24 dB), then scale by how much sound
        // reaches the listener's space.
        let reaches = if source_zone == listener_zone {
            1.0
        } else {
            bands[1]
        };
        let compensation = if gain > 0.0 {
            (1.0 / gain).clamp(1.0, 16.0)
        } else {
            1.0
        };
        let reverb_send = if source.spatial {
            (acoustic.reverb_send.clamp(0.0, 1.0) * reaches * compensation).clamp(0.0, 16.0)
        } else {
            0.0
        };
        let doppler = if source.spatial {
            self.settings.doppler(
                acoustic.doppler,
                (source.position, source.velocity),
                (listener.position, listener.velocity),
            )
        } else {
            1.0
        };
        let path = AcousticPath {
            visibility: v,
            transmission,
            reverb_send,
            air,
        };
        let parameters = VoiceParameters {
            gain: (source.gain * gain).clamp(0.0, 16.0),
            rate: (source.pitch * doppler).clamp(0.03125, 32.0),
            direction,
            path,
        };
        let record = OcclusionRecord {
            emitter: e.label.to_owned(),
            occluded: v < OCCLUDED_BELOW,
            visibility: v,
            transmission,
            air,
            reverb_send,
            zone: source_zone.map(|z| self.zones.zone(z).name.clone()),
            portals,
            blockers: occlusion
                .blockers
                .iter()
                .map(|b| Blocker {
                    entity: scene.label(b.surface),
                    material: material_name(&b.material).to_owned(),
                    distance: b.distance,
                    transmission: b.material.transmission,
                })
                .collect(),
        };
        Ok(Evaluation {
            parameters,
            record,
            distance: straight,
            attenuation: gain,
        })
    }

    fn zone_index(&self, name: &str) -> Option<usize> {
        (0..self.zones.len()).find(|&z| self.zones.zone(z).name == name)
    }
}
