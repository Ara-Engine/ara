//! Protocol 1 preserves the complete BC7 search with compile-time mode kernels.
use ara_gpu::{
    Access, Barrier, Buffer, Pipeline, Resource, RootConstants,
    vulkan::{Command, Recorder},
};

pub(super) const SCRATCH_BYTES: usize = 1552;
#[derive(Clone, Copy)]
pub(super) enum Pipelines<'a> {
    Single(Pipeline),
    Bc7 {
        modes: &'a [Pipeline; 8],
        select: Pipeline,
        reduce: Pipeline,
    },
}
impl Pipelines<'_> {
    pub(super) fn split(&self) -> bool {
        matches!(self, Self::Bc7 { .. })
    }
    pub(super) fn record(
        &self,
        commands: &mut Recorder,
        mut root: [u32; 8],
        scratch: Option<(Buffer, u32)>,
    ) {
        let count = root[2];
        let stage = |commands: &mut Recorder, pipeline, root: [u32; 8], groups| {
            commands.push(Command::Pipeline(pipeline));
            commands.push(Command::Constants(RootConstants { words: root }));
            commands.push(Command::Dispatch([groups, 1, 1]));
        };
        match *self {
            Self::Single(pipeline) => stage(commands, pipeline, root, count),
            Self::Bc7 {
                modes,
                select,
                reduce,
            } => {
                let (scratch, slot) = scratch.expect("split dispatch allocated scratch");
                root[4] = slot;
                root[7] = 1;
                let counts = if root[3] == 1 {
                    [8, 16, 16, 16, 8, 4, 1, 16]
                } else {
                    [16, 64, 64, 64, 8, 4, 1, 64]
                };
                let barrier = |commands: &mut Recorder| {
                    commands.barriers([Barrier {
                        resource: Resource::Buffer(scratch),
                        before: Access::Storage,
                        after: Access::Storage,
                        handoff: None,
                    }])
                };
                for (mode, &pipeline) in modes.iter().enumerate() {
                    stage(
                        commands,
                        pipeline,
                        root,
                        (count * counts[mode]).div_ceil(64),
                    );
                    barrier(commands);
                }
                stage(commands, select, root, count);
                barrier(commands);
                root[6] = 1;
                let finalists = if root[3] == 1 { 9 } else { 17 };
                for &pipeline in modes {
                    stage(commands, pipeline, root, (count * finalists).div_ceil(64));
                    barrier(commands);
                }
                stage(commands, reduce, root, count);
                barrier(commands);
            }
        }
    }
}
