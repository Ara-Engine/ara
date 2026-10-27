use super::*;
impl Residency {
    pub(super) fn record(&mut self, gpu: &GpuDevice, end: bool) -> Result<CommandList> {
        let mut graph = Graph::default();
        let ids: Vec<_> = self
            .buffers
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                graph.import(
                    Resource::Buffer(b),
                    if i == 4 {
                        State {
                            access: Access::HostWrite,
                            queue: Queue::Graphics,
                        }
                    } else {
                        self.previous.get(i).copied().unwrap_or(State {
                            access: if [0, 1, 3].contains(&i) {
                                Access::ShaderRead
                            } else {
                                Access::Undefined
                            },
                            queue: Queue::Graphics,
                        })
                    },
                )
            })
            .collect();
        if end {
            // Material consumers leave feedback/diagnostics in Storage and all
            // borrowed source resources in ShaderRead, exactly as Binding declares.
            graph = Graph::default();
            let ids: Vec<_> = self
                .buffers
                .iter()
                .enumerate()
                .map(|(i, &b)| {
                    graph.import(
                        Resource::Buffer(b),
                        State {
                            access: match i {
                                3 | 5 => Access::Storage,
                                4 => Access::ShaderRead,
                                6 => Access::HostRead,
                                _ => Access::ShaderRead,
                            },
                            queue: Queue::Graphics,
                        },
                    )
                })
                .collect();
            graph.pass(
                "Texture.FeedbackCopy",
                Queue::Graphics,
                [
                    (ids[3], Access::CopySource),
                    (ids[6], Access::CopyDestination),
                ],
            );
            graph.pass(
                "Texture.FeedbackReady",
                Queue::Graphics,
                [(ids[3], Access::Storage), (ids[6], Access::HostRead)],
            );
        } else {
            if self.pending > 0 {
                graph.pass(
                    "Texture.Upload",
                    Queue::Graphics,
                    [(ids[2], Access::Storage), (ids[4], Access::ShaderRead)],
                );
                graph.pass(
                    "Texture.Publish",
                    Queue::Graphics,
                    [
                        (ids[2], Access::ShaderRead),
                        (ids[1], Access::Storage),
                        (ids[4], Access::ShaderRead),
                    ],
                );
            }
            graph.pass(
                "Texture.Reset",
                Queue::Graphics,
                [(ids[3], Access::Storage)],
            );
            graph.pass(
                "Texture.SampleReady",
                Queue::Graphics,
                [
                    (ids[0], Access::ShaderRead),
                    (ids[1], Access::ShaderRead),
                    (ids[2], Access::ShaderRead),
                    (ids[3], Access::Storage),
                    (ids[5], Access::Storage),
                ],
            );
        }
        let compiled = graph.compile()?;
        self.previous = compiled.final_states;
        let mut recorder = gpu.begin(Queue::Graphics);
        for pass in compiled.passes {
            recorder.barriers(pass.before);
            let pipeline = match pass.name.as_str() {
                "Texture.Upload" => Some(0),
                "Texture.Publish" => Some(1),
                "Texture.Reset" => Some(2),
                _ => None,
            };
            if let Some(index) = pipeline {
                recorder.push(Command::BeginPass(pass.name));
                recorder.push(Command::Pipeline(self.pipelines[index]));
                recorder.push(Command::Constants(RootConstants {
                    words: [
                        self.slots[0],
                        self.slots[1],
                        self.slots[2],
                        self.slots[3],
                        self.slots[4],
                        self.pending,
                        self.catalog.page_count() as u32,
                        self.catalog.sources.len() as u32,
                    ],
                }));
                recorder.push(Command::Dispatch([
                    if index == 0 {
                        self.pending
                    } else if index == 1 {
                        self.pending.div_ceil(64)
                    } else {
                        2
                    },
                    1,
                    1,
                ]));
                recorder.push(Command::EndPass);
            } else if pass.name == "Texture.FeedbackCopy" {
                recorder.push(Command::BeginPass(pass.name));
                recorder.push(Command::CopyBuffer {
                    source: self.buffers[3],
                    destination: self.buffers[6],
                    size: (self.catalog.page_count() + 128) as u64 * 4,
                });
                recorder.push(Command::EndPass);
            }
            recorder.barriers(pass.after);
        }
        Ok(recorder.finish())
    }
}
