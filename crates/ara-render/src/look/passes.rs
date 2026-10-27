//! Look passes in the renderer's frame graph: the sky and fog before lighting, depth of
//! field before the temporal resolve, motion blur tiles, exposure, bloom and display
//! grading after it.
use super::gi::DISPATCH_PROBES;
use super::gpu::{
    AO_DENOISED, AO_TRACE, BLOOM_DOWN, BLOOM_UP, DOF_COMPOSITE, DOF_GATHERED, DOF_PREPARED,
    FROXELS, GI_DIFFUSE, GI_SPECULAR, LookGpu, LookIds, MOTION_NEIGHBOURS, MOTION_TILES,
    MULTISCATTER, SKY_IRRADIANCE, SKY_PREFILTER, SKY_VIEW, SSR_PYRAMID, SSR_TRACE, SkyKey,
    TRANSMITTANCE,
};
use crate::graph::{Graph, PassId, ResourceId};
use ara_gpu::{
    Access, Queue, RootConstants,
    vulkan::{Command, Recorder},
};

/// One look dispatch; `Bloom` carries its pyramid pass (0..9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LookPass {
    Transmittance,
    MultiScatter,
    SkyView,
    SkyIrradiance,
    SkyPrefilter,
    Fog,
    /// Metering: 0 bins the history, 1 meters the bins into the exposure state.
    Exposure(u32),
    Bloom(u32),
    Post,
    /// Depth of field: 0 prepares, 1 gathers (half resolution), 2 composites.
    Dof(u32),
    /// Motion blur velocity maxima: 0 per tile, 1 per neighbourhood.
    Motion(u32),
    GiRelight,
    /// Clears a fresh probe volume (`GiGpu::fresh`).
    GiClear,
    /// Scatters staged probe write `n` (`LookGpu::probe_writes`).
    GiScatter(u32),
    AoTrace,
    AoDenoise,
    GiGather,
    /// Reflection colour pyramid level (1..4) from the previous history.
    SsrPyramid(u32),
    SsrTrace,
    /// Measures the ambient light at each reflection capture against its bake
    /// (`look::captures`); the mask marks bakes that landed since the last one.
    CaptureRefit(u32),
    /// Lists the captures each 16x16 render tile can need (`look::captures`).
    CaptureTiles,
    /// The horizon's sun visibility (`look::horizon`).
    HorizonSun,
    /// The horizon's sky visibility (`look::horizon`).
    HorizonSky,
    /// Marks the virtual shadow pages this frame's pixels sample (`look::vsm`).
    VsmMark,
    /// Resolves the G-buffer's virtual shadow page visibility into the mask lighting
    /// reads (`look::vsm`).
    VsmProject,
}

/// The frame records look passes run with: scene passes at the render size; the
/// post chain (exposure, bloom, grading) at the display size, after the temporal
/// resolve (`Renderer::resize_upscaled`).
#[derive(Clone, Copy)]
pub(crate) struct Frames {
    /// Frame record (bindless index) and extent of the scene passes.
    pub render: (u32, [u32; 2]),
    /// Frame record and extent of the post chain.
    pub display: (u32, [u32; 2]),
}

impl LookGpu {
    /// Explicit capture boundaries may drain a loading reservation synchronously.
    pub(crate) fn record_probe_clear(&self, recorder: &mut Recorder) {
        self.record_scatter(recorder, LookPass::GiClear);
    }
    /// Declares the sky and fog passes. LUTs are rebuilt when the aerosols change,
    /// the sky IBL when the sun or sky changes, the sky view and froxels per frame.
    /// `shared` is the frame's conservative bindless reachability.
    pub(crate) fn sky_passes(
        &mut self,
        graph: &mut Graph,
        ids: &LookIds,
        shared: &[(ResourceId, Access)],
        sun: [f32; 6],
    ) -> Vec<(PassId, LookPass)> {
        let mut passes = Vec::new();
        let physical = self.look.sky.is_some();
        let mut add = |graph: &mut Graph, name: &str, pass, uses: Vec<(usize, Access)>| {
            let id = graph.pass(
                name,
                Queue::Graphics,
                uses.into_iter()
                    .map(|(i, a)| (ids.image(i), a))
                    .chain([(ids.frame, Access::ShaderRead)])
                    .chain(shared.iter().copied()),
            );
            passes.push((id, pass));
        };
        if let Some(sky) = self.look.sky {
            let key = SkyKey {
                aerosols: sky.aerosols,
                sun,
                clouds: [sky.cloud_coverage, sky.cloud_height],
            };
            if self.sky.is_none_or(|k| k.aerosols != key.aerosols) {
                add(
                    graph,
                    "Sky.Transmittance",
                    LookPass::Transmittance,
                    vec![(TRANSMITTANCE, Access::Storage)],
                );
                add(
                    graph,
                    "Sky.MultiScatter",
                    LookPass::MultiScatter,
                    vec![
                        (TRANSMITTANCE, Access::ShaderRead),
                        (MULTISCATTER, Access::Storage),
                    ],
                );
            }
            add(
                graph,
                "Sky.View",
                LookPass::SkyView,
                vec![
                    (TRANSMITTANCE, Access::ShaderRead),
                    (MULTISCATTER, Access::ShaderRead),
                    (SKY_VIEW, Access::Storage),
                ],
            );
            if self.sky != Some(key) {
                add(
                    graph,
                    "Sky.Irradiance",
                    LookPass::SkyIrradiance,
                    vec![
                        (SKY_VIEW, Access::ShaderRead),
                        (SKY_IRRADIANCE, Access::Storage),
                    ],
                );
                add(
                    graph,
                    "Sky.Prefilter",
                    LookPass::SkyPrefilter,
                    vec![
                        (SKY_VIEW, Access::ShaderRead),
                        (SKY_PREFILTER, Access::Storage),
                    ],
                );
            }
            self.sky = Some(key);
        }
        if physical || self.look.fog.is_some() {
            add(
                graph,
                "Fog.Froxels",
                LookPass::Fog,
                if physical {
                    vec![
                        (TRANSMITTANCE, Access::ShaderRead),
                        (MULTISCATTER, Access::ShaderRead),
                        (FROXELS, Access::Storage),
                    ]
                } else {
                    vec![(FROXELS, Access::Storage)]
                },
            );
        }
        if let Some(gi) = ids.gi {
            // New probe records land before the relight reads them.
            if self.gi.as_ref().is_some_and(|g| g.fresh) {
                let id = graph.pass(
                    "GI.Clear",
                    Queue::Graphics,
                    gi.map(|r| (r, Access::Storage))
                        .into_iter()
                        .chain(shared.iter().copied()),
                );
                passes.push((id, LookPass::GiClear));
            }
            for (n, &staging) in ids.probe_writes.iter().enumerate() {
                let id = graph.pass(
                    "GI.Scatter",
                    Queue::Graphics,
                    [
                        (gi[0], Access::Storage),
                        (gi[1], Access::Storage),
                        (gi[2], Access::Storage),
                        (staging, Access::ShaderRead),
                    ]
                    .into_iter()
                    .chain(shared.iter().copied()),
                );
                passes.push((id, LookPass::GiScatter(n as u32)));
            }
            if self.gi.as_ref().is_some_and(|g| g.clearing_in_slices()) {
                return passes;
            }
            let id = graph.pass(
                "GI.Relight",
                Queue::Graphics,
                [
                    (gi[0], Access::ShaderRead),
                    (gi[1], Access::ShaderRead),
                    (gi[2], Access::ShaderRead),
                    (gi[3], Access::ShaderRead),
                    (gi[4], Access::Storage),
                    (ids.frame, Access::ShaderRead),
                ]
                .into_iter()
                .chain(
                    [TRANSMITTANCE, SKY_VIEW]
                        .into_iter()
                        .filter(|_| physical)
                        .map(|i| (ids.image(i), Access::ShaderRead)),
                )
                .chain(shared.iter().copied()),
            );
            passes.push((id, LookPass::GiRelight));
        }
        passes
    }

    /// Screen-space reflections before lighting: a colour pyramid of the previous
    /// resolved frame (`history`; none on the first frame) and the half-resolution
    /// trace. `reads` must cover depth, normals, world positions and the frame.
    pub(crate) fn ssr_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        history: Option<ResourceId>,
        reads: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        let (Some(_), Some(history)) = (self.quality.reflections, history) else {
            return Vec::new();
        };
        let mut passes = Vec::new();
        for level in 1..=4u32 {
            const NAMES: [&str; 4] = [
                "SSR.Pyramid1",
                "SSR.Pyramid2",
                "SSR.Pyramid3",
                "SSR.Pyramid4",
            ];
            let id = graph.pass(
                NAMES[level as usize - 1],
                Queue::Graphics,
                [
                    (history, Access::ShaderRead),
                    (ids.image(SSR_PYRAMID), Access::Storage),
                    (ids.frame, Access::ShaderRead),
                    (ids.state, Access::ShaderRead),
                ]
                .into_iter()
                .chain(reads.iter().copied()),
            );
            passes.push((id, LookPass::SsrPyramid(level)));
        }
        let id = graph.pass(
            "SSR.Trace",
            Queue::Graphics,
            [
                (ids.image(SSR_PYRAMID), Access::ShaderRead),
                (ids.image(SSR_TRACE), Access::Storage),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(reads.iter().copied()),
        );
        passes.push((id, LookPass::SsrTrace));
        passes
    }

    /// The half-resolution probe gather (after the relight); `reads` must cover the
    /// G-buffer world positions and normals.
    pub(crate) fn gather_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        reads: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        let Some(gi) = ids.gi else {
            return Vec::new();
        };
        let id = graph.pass(
            "GI.Gather",
            Queue::Graphics,
            [
                (gi[1], Access::ShaderRead),
                (gi[2], Access::ShaderRead),
                (gi[4], Access::ShaderRead),
                (ids.image(GI_DIFFUSE), Access::Storage),
                (ids.image(GI_SPECULAR), Access::Storage),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(reads.iter().copied()),
        );
        vec![(id, LookPass::GiGather)]
    }

    /// Ambient occlusion after the G-buffer: half-resolution GTAO, or on the High/Epic
    /// tier the visibility bitmask with near-field bounce (`bitmask_ao`), then a
    /// depth-aware denoise. `reads` must cover depth, normals, materials (cutouts) and
    /// the frame; `reflected` when this frame built the reflection colour pyramid,
    /// which the bitmask's bounce reads (declare these passes after it).
    pub(crate) fn ao_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        reads: &[(ResourceId, Access)],
        reflected: bool,
    ) -> Vec<(PassId, LookPass)> {
        if self.quality.ambient_occlusion.is_none() {
            return Vec::new();
        }
        let common = [(ids.frame, Access::ShaderRead)];
        let bitmask = self.bitmask_ao();
        let trace = graph.pass(
            if bitmask { "AO.Bitmask" } else { "AO.GTAO" },
            Queue::Graphics,
            [(ids.image(AO_TRACE), Access::Storage)]
                .into_iter()
                .chain((bitmask && reflected).then(|| (ids.image(SSR_PYRAMID), Access::ShaderRead)))
                .chain(common)
                .chain(reads.iter().copied()),
        );
        let denoise = graph.pass(
            "AO.Denoise",
            Queue::Graphics,
            [
                (ids.image(AO_TRACE), Access::ShaderRead),
                (ids.image(AO_DENOISED), Access::Storage),
            ]
            .into_iter()
            .chain(common)
            .chain(reads.iter().copied()),
        );
        vec![(trace, LookPass::AoTrace), (denoise, LookPass::AoDenoise)]
    }

    /// Images the lighting and transparency passes sample; `reflected` when this
    /// frame traced screen-space reflections (they need last frame's history).
    pub(crate) fn lighting_reads(
        &self,
        ids: &LookIds,
        reflected: bool,
    ) -> Vec<(ResourceId, Access)> {
        let mut reads = vec![(ids.frame, Access::ShaderRead)];
        if self.look.sky.is_some() {
            reads.extend(
                [TRANSMITTANCE, SKY_VIEW, SKY_IRRADIANCE, SKY_PREFILTER]
                    .map(|i| (ids.image(i), Access::ShaderRead)),
            );
        }
        if self.look.sky.is_some() || self.look.fog.is_some() {
            reads.push((ids.image(FROXELS), Access::ShaderRead));
        }
        if let Some(gi) = ids.gi {
            reads.extend([1, 2, 4].map(|i| (gi[i], Access::ShaderRead)));
            reads.extend([GI_DIFFUSE, GI_SPECULAR].map(|i| (ids.image(i), Access::ShaderRead)));
        }
        if self.quality.ambient_occlusion.is_some() {
            reads.push((ids.image(AO_DENOISED), Access::ShaderRead));
        }
        if reflected {
            reads.push((ids.image(SSR_TRACE), Access::ShaderRead));
        }
        reads
    }

    /// Declares depth of field (`frame_dof`) before the temporal resolve: prepare and
    /// gather at half the render size, then the composite the resolve reads instead of
    /// `hdr` (`DOF_COMPOSITE`). `depth` is the frame's depth target.
    pub(crate) fn dof_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        hdr: ResourceId,
        depth: ResourceId,
        shared: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        let read = Access::ShaderRead;
        let stages = [
            (
                "DoF.Prepare",
                vec![
                    (hdr, read),
                    (depth, read),
                    (ids.image(DOF_PREPARED), Access::Storage),
                ],
            ),
            (
                "DoF.Gather",
                vec![
                    (ids.image(DOF_PREPARED), read),
                    (ids.image(DOF_GATHERED), Access::Storage),
                ],
            ),
            (
                "DoF.Composite",
                vec![
                    (hdr, read),
                    (depth, read),
                    (ids.image(DOF_GATHERED), read),
                    (ids.image(DOF_COMPOSITE), Access::Storage),
                ],
            ),
        ];
        stages
            .into_iter()
            .enumerate()
            .map(|(n, (name, uses))| {
                let id = graph.pass(
                    name,
                    Queue::Graphics,
                    uses.into_iter()
                        .chain([(ids.frame, read)])
                        .chain(shared.iter().copied()),
                );
                (id, LookPass::Dof(n as u32))
            })
            .collect()
    }

    /// Declares exposure, bloom and the display pass from `input` (the resolved
    /// history) into `output` (SDR storage). With `motion` (this
    /// frame's motion vectors and depth; `frame_motion`), the velocity tiles first and
    /// the post pass's motion variant.
    pub(crate) fn post_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        input: ResourceId,
        output: ResourceId,
        motion: Option<[ResourceId; 2]>,
        shared: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        let mut passes = Vec::new();
        let common = [(ids.frame, Access::ShaderRead), (input, Access::ShaderRead)];
        let mut add = |graph: &mut Graph, name: &str, pass, uses: Vec<(ResourceId, Access)>| {
            let id = graph.pass(
                name,
                Queue::Graphics,
                uses.into_iter().chain(common).chain(shared.iter().copied()),
            );
            passes.push((id, pass));
        };
        for (pass, name) in ["Exposure.Histogram", "Exposure.Meter"].iter().enumerate() {
            add(
                graph,
                name,
                LookPass::Exposure(pass as u32),
                vec![(ids.state, Access::Storage)],
            );
        }
        let bloom = self.effective().bloom > 0.;
        if bloom {
            const NAMES: [&str; 9] = [
                "Bloom.Down2",
                "Bloom.Down3",
                "Bloom.Down4",
                "Bloom.Down5",
                "Bloom.Down6",
                "Bloom.Up5",
                "Bloom.Up4",
                "Bloom.Up3",
                "Bloom.Up2",
            ];
            for (pass, name) in NAMES.iter().enumerate() {
                let uses = match pass {
                    0 => vec![
                        (ids.state, Access::ShaderRead),
                        (ids.image(BLOOM_DOWN), Access::Storage),
                    ],
                    1..=4 => vec![(ids.image(BLOOM_DOWN), Access::Storage)],
                    _ => vec![
                        (ids.image(BLOOM_DOWN), Access::ShaderRead),
                        (ids.image(BLOOM_UP), Access::Storage),
                    ],
                };
                add(graph, name, LookPass::Bloom(pass as u32), uses);
            }
        }
        let mut uses = vec![
            (ids.state, Access::ShaderRead),
            (ids.lut, Access::ShaderRead),
            (output, Access::Storage),
        ];
        if bloom {
            uses.push((ids.image(BLOOM_UP), Access::ShaderRead));
        }
        if let Some([vectors, depth]) = motion {
            let read = Access::ShaderRead;
            let (tiles, neighbours) = (ids.image(MOTION_TILES), ids.image(MOTION_NEIGHBOURS));
            add(
                graph,
                "MotionBlur.Tiles",
                LookPass::Motion(0),
                vec![(vectors, read), (tiles, Access::Storage)],
            );
            add(
                graph,
                "MotionBlur.Neighbours",
                LookPass::Motion(1),
                vec![(tiles, read), (neighbours, Access::Storage)],
            );
            uses.extend([(neighbours, read), (vectors, read), (depth, read)]);
        }
        add(graph, "Post.Grade.ToneMap", LookPass::Post, uses);
        passes
    }

    /// Records one look dispatch. `input` is the sampled index of the scene HDR
    /// (post passes; the history for the reflection pyramid), `exposed` whether it is
    /// the pre-exposed TAA history rather than linear HDR.
    pub(crate) fn record(
        &self,
        recorder: &mut Recorder,
        pass: LookPass,
        frames: Frames,
        input: u32,
        exposed: bool,
    ) {
        let p = &self.pipelines;
        let (frame_index, size) = match pass {
            LookPass::Exposure(_) | LookPass::Bloom(_) | LookPass::Post | LookPass::Motion(_) => {
                frames.display
            }
            _ => frames.render,
        };
        if let LookPass::GiClear | LookPass::GiScatter(_) = pass {
            self.record_scatter(recorder, pass);
            return;
        }
        let groups = |[w, h]: [u32; 2]| [w.div_ceil(8), h.div_ceil(8), 1];
        let (pipeline, argument, dispatch) = match pass {
            LookPass::Transmittance => (p.transmittance, 0, groups([256, 64])),
            LookPass::MultiScatter => (p.multiscatter, 0, groups([32, 32])),
            LookPass::SkyView => (p.sky_view, 0, groups([192, 108])),
            LookPass::SkyIrradiance => (p.sky_irradiance, 0, groups([32, 16])),
            LookPass::SkyPrefilter => (p.sky_prefilter, 0, groups([256, 640])),
            LookPass::Fog => (p.fog, 0, groups([64, 36])),
            // One grid sample per thread (128x72), then one metering group.
            LookPass::Exposure(n) => (p.exposure, n, [if n == 0 { 36 } else { 1 }, 1, 1]),
            LookPass::Bloom(n) => {
                // Levels 2..6 (quarter resolution down), as in look-post.slang.
                let level = if n < 5 { n } else { 8 - n } + 2;
                (p.bloom, n, groups(size.map(|v| (v >> level).max(1))))
            }
            LookPass::Post => (
                if self.frame_motion {
                    p.post_motion
                } else {
                    p.post
                },
                0,
                groups(size),
            ),
            // Prepare and gather at half the render size, the composite at full.
            LookPass::Dof(n) => (
                p.dof,
                n,
                groups(if n == 2 {
                    size
                } else {
                    size.map(|v| v.div_ceil(2))
                }),
            ),
            LookPass::Motion(n) => (p.motion, n, groups(size.map(|v| v.div_ceil(16)))),
            LookPass::AoTrace => (
                if self.bitmask_ao() {
                    p.ao_bitmask
                } else {
                    p.ao_trace
                },
                0,
                groups(size.map(|v| v.div_ceil(2))),
            ),
            LookPass::AoDenoise => (
                if self.bitmask_ao() {
                    p.ao_bitmask_denoise
                } else {
                    p.ao_denoise
                },
                0,
                groups(size.map(|v| v.div_ceil(2))),
            ),
            LookPass::GiGather => (p.gi_gather, 0, groups(size.map(|v| v.div_ceil(2)))),
            LookPass::SsrPyramid(level) => (
                p.ssr_pyramid,
                level - 1,
                groups(size.map(|v| (v >> level).max(1))),
            ),
            LookPass::SsrTrace => (p.ssr_trace, 0, groups(size.map(|v| v.div_ceil(2)))),
            // One thread per capture (look::captures::MAX_CAPTURES).
            LookPass::CaptureRefit(masks) => (p.capture_refit, masks, [1, 1, 1]),
            LookPass::CaptureTiles => (
                p.capture_tiles,
                0,
                [size[0].div_ceil(16), size[1].div_ceil(16), 1],
            ),
            // With virtual shadow maps, hits compare against their pages first.
            LookPass::GiRelight => (
                if self.vsm.is_some() {
                    p.gi_relight_vsm
                } else {
                    p.gi_relight
                },
                0,
                [
                    self.gi
                        .as_ref()
                        .map_or(0, |gi| gi.relight_groups(self.quality.gi_budget)),
                    1,
                    1,
                ],
            ),
            LookPass::HorizonSun => (
                p.horizon_sun,
                0,
                groups([self.horizon.as_ref().map_or(0, |h| h.resolution); 2]),
            ),
            LookPass::HorizonSky => (
                p.horizon_sky,
                0,
                groups([self.horizon.as_ref().map_or(0, |h| h.sky_resolution()); 2]),
            ),
            LookPass::VsmMark => (p.vsm_mark, 0, groups(size.map(|v| v.div_ceil(4)))),
            LookPass::VsmProject => (p.vsm_project, 0, groups(size)),
            LookPass::GiClear | LookPass::GiScatter(_) => return,
        };
        recorder.push(Command::Pipeline(pipeline));
        recorder.push(Command::Constants(RootConstants {
            words: [frame_index, input, argument, 0, 0, 0, 0, u32::from(exposed)],
        }));
        recorder.push(Command::Dispatch(dispatch));
    }

    /// Records `giScatterMain`: a fresh volume's clear (every probe inactive and
    /// dark) or staged probe write `n`, at most [`DISPATCH_PROBES`] per dispatch.
    fn record_scatter(&self, recorder: &mut Recorder, pass: LookPass) {
        let Some(gi) = &self.gi else {
            return;
        };
        let (staging, base, count, clear) = match pass {
            LookPass::GiScatter(n) => match self.probe_writes.get(n as usize) {
                Some((staging, count, _)) => (staging.index, 0, *count, false),
                None => return,
            },
            _ => {
                let (first, count) = gi.clear_window();
                (0, first, count, true)
            }
        };
        let mut words = [
            0,
            staging,
            gi.indices[0],
            gi.indices[1],
            gi.moments_storage,
            gi.atlas | (u32::from(clear) << 31),
            gi.irradiance_storage[0],
            gi.irradiance_storage[1],
        ];
        recorder.push(Command::Pipeline(self.pipelines.gi_scatter));
        for first in (0..count).step_by(DISPATCH_PROBES as usize) {
            words[0] = base + first;
            recorder.push(Command::Constants(RootConstants { words }));
            recorder.push(Command::Dispatch([
                (count - first).min(DISPATCH_PROBES),
                1,
                1,
            ]));
        }
    }
}
