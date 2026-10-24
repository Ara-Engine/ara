//! Look passes in the renderer's frame graph: the sky and fog before lighting,
//! exposure, bloom and display grading after temporal resolve.
use super::gpu::{
    BLOOM_DOWN, BLOOM_UP, FROXELS, LookGpu, LookIds, MULTISCATTER, SKY_IRRADIANCE, SKY_PREFILTER,
    SKY_VIEW, SkyKey, TRANSMITTANCE,
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
                    .map(|(i, a)| (ids.images[i], a))
                    .chain([(ids.frame, Access::ShaderRead)])
                    .chain(shared.iter().copied()),
            );
            passes.push((id, pass));
        };
        if let Some(sky) = self.look.sky {
            let key = SkyKey {
                aerosols: sky.aerosols,
                sun,
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
                    (gi[3], Access::Storage),
                    (ids.frame, Access::ShaderRead),
                ]
                .into_iter()
                .chain(
                    [TRANSMITTANCE, SKY_VIEW]
                        .into_iter()
                        .filter(|_| physical)
                        .map(|i| (ids.images[i], Access::ShaderRead)),
                )
                .chain(shared.iter().copied()),
            );
            passes.push((id, LookPass::GiRelight));
        }
        passes
    }

    /// Images the lighting and transparency passes sample.
    pub(crate) fn lighting_reads(&self, ids: &LookIds) -> Vec<(ResourceId, Access)> {
        let mut reads = vec![(ids.frame, Access::ShaderRead)];
        if self.look.sky.is_some() {
            reads.extend(
                [TRANSMITTANCE, SKY_VIEW, SKY_IRRADIANCE, SKY_PREFILTER]
                    .map(|i| (ids.images[i], Access::ShaderRead)),
            );
        }
        if self.look.sky.is_some() || self.look.fog.is_some() {
            reads.push((ids.images[FROXELS], Access::ShaderRead));
        }
        if let Some(gi) = ids.gi {
            reads.extend([(gi[1], Access::ShaderRead), (gi[3], Access::ShaderRead)]);
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
        if self.look.bloom > 0. {
            const NAMES: [&str; 9] = [
                "Bloom.Down1",
                "Bloom.Down2",
                "Bloom.Down3",
                "Bloom.Down4",
                "Bloom.Down5",
                "Bloom.Up4",
                "Bloom.Up3",
                "Bloom.Up2",
                "Bloom.Up1",
            ];
            for (pass, name) in NAMES.iter().enumerate() {
                let uses = match pass {
                    0 => vec![
                        (ids.state, Access::ShaderRead),
                        (ids.images[BLOOM_DOWN], Access::Storage),
                    ],
                    1..=4 => vec![(ids.images[BLOOM_DOWN], Access::Storage)],
                    _ => vec![
                        (ids.images[BLOOM_DOWN], Access::ShaderRead),
                        (ids.images[BLOOM_UP], Access::Storage),
                    ],
                };
                add(graph, name, LookPass::Bloom(pass as u32), uses);
            }
        }
        let mut uses = vec![(ids.state, Access::ShaderRead), (output, Access::Storage)];
        if self.look.bloom > 0. {
            uses.push((ids.images[BLOOM_UP], Access::ShaderRead));
        }
        add(graph, "Post.Grade.ToneMap", LookPass::Post, uses);
        passes
    }

    /// Records one look dispatch. `input` is the sampled index of the scene HDR
    /// (post passes); `size` is the render extent.
    pub(crate) fn record(
        &self,
        recorder: &mut Recorder,
        pass: LookPass,
        frame_index: u32,
        input: u32,
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
                let level = if n < 5 { n + 1 } else { 9 - n };
                (p.bloom, n, groups(size.map(|v| (v >> level).max(1))))
            }
            LookPass::Post => (p.post, 0, groups(size)),
            LookPass::GiRelight => (
                p.gi_relight,
                0,
                [self.gi.as_ref().map_or(0, |gi| gi.probes), 1, 1],
            ),
        };
        recorder.push(Command::Pipeline(pipeline));
        recorder.push(Command::Constants(RootConstants {
            words: [frame_index, input, argument, 0, 0, 0, 0, 0],
        }));
        recorder.push(Command::Dispatch(dispatch));
    }
}
