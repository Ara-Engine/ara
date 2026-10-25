//! Mode-specialized HDR encoding, preserving the frozen (error, words) winner.
use ara_gpu::{
    Access, Barrier, Buffer, Pipeline, Resource, RootConstants,
    vulkan::{Command, Recorder},
};

pub(super) const SCRATCH_BYTES: usize = 344;

#[derive(Clone, Copy)]
pub(super) struct Bc6Pipelines<'a> {
    pub(super) modes: &'a [Pipeline; 14],
    pub(super) balanced: Option<(&'a [Pipeline; 4], Pipeline)>,
    pub(super) reduce: Pipeline,
}

impl Bc6Pipelines<'_> {
    pub(super) fn record(
        &self,
        commands: &mut Recorder,
        mut root: [u32; 8],
        scratch: (Buffer, u32),
    ) {
        let (buffer, slot) = scratch;
        root[4] = slot;
        root[7] = 1;
        let count = root[2];
        let stage = |commands: &mut Recorder, pipeline, groups| {
            commands.push(Command::Pipeline(pipeline));
            commands.push(Command::Constants(RootConstants { words: root }));
            commands.push(Command::Dispatch([groups, 1, 1]));
            commands.barriers([Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::Storage,
                after: Access::Storage,
                handoff: None,
            }]);
        };
        match root[3] {
            0 => stage(commands, self.modes[10], count.div_ceil(64)),
            1 => {
                let (modes, rank) = self.balanced.expect("Balanced pipelines were loaded");
                stage(commands, rank, count);
                for &pipeline in modes {
                    stage(commands, pipeline, count.div_ceil(8));
                }
                for &pipeline in &self.modes[10..14] {
                    stage(commands, pipeline, count.div_ceil(64));
                }
            }
            _ => {
                for (mode, &pipeline) in self.modes.iter().enumerate() {
                    stage(
                        commands,
                        pipeline,
                        if mode < 10 { count } else { count.div_ceil(64) },
                    );
                }
            }
        }
        stage(commands, self.reduce, count.div_ceil(64));
    }
}
