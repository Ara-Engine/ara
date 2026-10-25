//! Protocol 2 skips work only when mode 6 has already proved the exact zero winner.
use ara_gpu::{
    Access, Barrier, Buffer, Pipeline, Resource, RootConstants,
    vulkan::{Command, Recorder},
};

pub(super) const SCRATCH_BYTES: usize = 1552;
#[derive(Clone, Copy)]
pub(super) enum Pipelines<'a> {
    Single(Pipeline),
    Bc6h(crate::bc6_split::Bc6Pipelines<'a>),
    Bc7 {
        modes: &'a [Pipeline; 8],
        select: Pipeline,
        reduce: Pipeline,
    },
}
impl Pipelines<'_> {
    pub(super) fn split(&self) -> bool {
        !matches!(self, Self::Single(_))
    }
    pub(super) fn scratch_bytes(&self) -> usize {
        match self {
            Self::Single(_) => 0,
            Self::Bc6h(_) => crate::bc6_split::SCRATCH_BYTES,
            Self::Bc7 { .. } => SCRATCH_BYTES,
        }
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
            Self::Bc6h(pipelines) => {
                pipelines.record(commands, root, scratch.expect("BC6H scratch was allocated"))
            }
            Self::Bc7 {
                modes,
                select,
                reduce,
            } => {
                let (scratch, slot) = scratch.expect("split dispatch allocated scratch");
                root[4] = slot;
                root[7] = 2;
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
                // Mode 6 candidate zero establishes the global minimum before
                // other modes inspect its score. Every dispatch has a storage barrier.
                for mode in [6, 0, 1, 2, 3, 4, 5, 7] {
                    stage(
                        commands,
                        modes[mode],
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
