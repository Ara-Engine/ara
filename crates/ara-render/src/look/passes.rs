//! Look passes in the renderer's frame graph: the sky and fog before lighting,
//! exposure, bloom and display grading after temporal resolve.
use super::gpu::{
    AO_DENOISED, AO_TRACE, BLOOM_DOWN, BLOOM_UP, FROXELS, GI_DIFFUSE, GI_SPECULAR, LookGpu,
    LookIds, MULTISCATTER, SKY_IRRADIANCE, SKY_PREFILTER, SKY_VIEW, SSR_PYRAMID, SSR_TRACE, SkyKey,
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
    Exposure,
    Bloom(u32),
    Post,
    GiRelight,
    AoTrace,
    AoDenoise,
    GiGather,
    /// Reflection colour pyramid level (1..4) from the previous history.
    SsrPyramid(u32),
    SsrTrace,
}

impl LookGpu {
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

    /// Ambient occlusion after the G-buffer: half-resolution GTAO, then a
    /// depth-aware denoise. `reads` must cover depth, normals and the frame.
    pub(crate) fn ao_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        reads: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        if self.quality.ambient_occlusion.is_none() {
            return Vec::new();
        }
        let common = [(ids.frame, Access::ShaderRead)];
        let trace = graph.pass(
            "AO.GTAO",
            Queue::Graphics,
            [(ids.image(AO_TRACE), Access::Storage)]
                .into_iter()
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

    /// Declares exposure, bloom and the display pass from `input` (linear HDR)
    /// into `output` (SDR storage).
    pub(crate) fn post_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        input: ResourceId,
        output: ResourceId,
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
        add(
            graph,
            "Exposure.Histogram",
            LookPass::Exposure,
            vec![(ids.state, Access::Storage)],
        );
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
        let mut uses = vec![(ids.state, Access::ShaderRead), (output, Access::Storage)];
        if bloom {
            uses.push((ids.image(BLOOM_UP), Access::ShaderRead));
        }
        add(graph, "Post.Grade.ToneMap", LookPass::Post, uses);
        passes
    }

    /// Records one look dispatch. `input` is the sampled index of the scene HDR
    /// (post passes; the history for the reflection pyramid), `exposed` whether it is
    /// the pre-exposed TAA history rather than linear HDR; `size` is the render extent.
    pub(crate) fn record(
        &self,
        recorder: &mut Recorder,
        pass: LookPass,
        frame_index: u32,
        input: u32,
        exposed: bool,
        size: [u32; 2],
    ) {
        let p = &self.pipelines;
        let groups = |[w, h]: [u32; 2]| [w.div_ceil(8), h.div_ceil(8), 1];
        let (pipeline, argument, dispatch) = match pass {
            LookPass::Transmittance => (p.transmittance, 0, groups([256, 64])),
            LookPass::MultiScatter => (p.multiscatter, 0, groups([32, 32])),
            LookPass::SkyView => (p.sky_view, 0, groups([192, 108])),
            LookPass::SkyIrradiance => (p.sky_irradiance, 0, groups([32, 16])),
            LookPass::SkyPrefilter => (p.sky_prefilter, 0, groups([256, 640])),
            LookPass::Fog => (p.fog, 0, groups([64, 36])),
            LookPass::Exposure => (p.exposure, 0, [1, 1, 1]),
            LookPass::Bloom(n) => {
                // Levels 2..6 (quarter resolution down), as in look-post.slang.
                let level = if n < 5 { n } else { 8 - n } + 2;
                (p.bloom, n, groups(size.map(|v| (v >> level).max(1))))
            }
            LookPass::Post => (p.post, 0, groups(size)),
            LookPass::AoTrace => (p.ao_trace, 0, groups(size.map(|v| v.div_ceil(2)))),
            LookPass::AoDenoise => (p.ao_denoise, 0, groups(size.map(|v| v.div_ceil(2)))),
            LookPass::GiGather => (p.gi_gather, 0, groups(size.map(|v| v.div_ceil(2)))),
            LookPass::SsrPyramid(level) => (
                p.ssr_pyramid,
                level - 1,
                groups(size.map(|v| (v >> level).max(1))),
            ),
            LookPass::SsrTrace => (p.ssr_trace, 0, groups(size.map(|v| v.div_ceil(2)))),
            LookPass::GiRelight => (
                p.gi_relight,
                0,
                [self.gi.as_ref().map_or(0, |gi| gi.probes), 1, 1],
            ),
        };
        recorder.push(Command::Pipeline(pipeline));
        recorder.push(Command::Constants(RootConstants {
            words: [frame_index, input, argument, 0, 0, 0, 0, u32::from(exposed)],
        }));
        recorder.push(Command::Dispatch(dispatch));
    }
}
