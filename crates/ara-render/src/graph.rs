//! Whole-resource graph v0. Declaration order defines successive writes.
//! Independent passes are stably topologically sorted; explicit edges may reorder
//! them. Reads before a first producer are legal and move after that producer.
use anyhow::{Result, bail, ensure};
use ara_gpu::{Access, Barrier, Handoff, Queue, Resource, vk, vulkan::VulkanDevice};
use std::collections::BTreeSet;

/// Resource identity inside one graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId(pub usize);
/// Pass identity inside one graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PassId(pub usize);
/// Initial or final resource ownership and access.
#[derive(Debug, Clone, Copy)]
pub struct State {
    /// Last access, or Undefined for new storage.
    pub access: Access,
    /// Current exclusive owner.
    pub queue: Queue,
}
/// An image allocated at graph instantiation, without transient aliasing in v0.
pub struct TransientImage {
    /// Pixel extent.
    pub size: [u32; 2],
    /// Attachment/storage format.
    pub format: vk::Format,
    /// All uses reachable through this graph.
    pub usage: vk::ImageUsageFlags,
}
struct GraphResource {
    resource: Resource,
    state: State,
}
struct Pass {
    name: String,
    queue: Queue,
    uses: Vec<(ResourceId, Access)>,
    after: BTreeSet<usize>,
}
/// Resource declarations and dependencies, independent of native API families.
#[derive(Default)]
pub struct Graph {
    resources: Vec<GraphResource>,
    passes: Vec<Pass>,
}
/// Scheduled pass with generated pre/post dependencies.
pub struct CompiledPass {
    /// Original pass identity, used to select the pass implementation.
    pub id: PassId,
    /// Diagnostics/timestamp name.
    pub name: String,
    /// Execution queue.
    pub queue: Queue,
    /// Incoming resource dependencies.
    pub before: Vec<Barrier>,
    /// Outgoing queue release operations.
    pub after: Vec<Barrier>,
    /// Earlier pass indices whose timelines this queue waits for.
    pub waits: Vec<usize>,
}
/// Fully ordered passes and final ownership, reusable by the next frame.
pub struct CompiledGraph {
    /// Topologically ordered passes.
    pub passes: Vec<CompiledPass>,
    /// State after all passes, indexed by ResourceId.
    pub final_states: Vec<State>,
}
impl Graph {
    /// Import owned storage with its actual last usage and queue.
    pub fn import(&mut self, resource: Resource, state: State) -> ResourceId {
        let id = ResourceId(self.resources.len());
        self.resources.push(GraphResource { resource, state });
        id
    }
    /// Allocate a transient image and import it in Undefined state.
    /// The caller retires the returned image after its graph submission.
    pub fn transient(
        &mut self,
        gpu: &mut VulkanDevice,
        desc: TransientImage,
        queue: Queue,
    ) -> Result<(ResourceId, ara_gpu::Image)> {
        let image = gpu.create_image(desc.size[0], desc.size[1], desc.format, desc.usage)?;
        Ok((
            self.import(
                Resource::Image(image),
                State {
                    access: Access::Undefined,
                    queue,
                },
            ),
            image,
        ))
    }
    /// Declare a pass's complete direct and bindless resource reachability.
    pub fn pass(
        &mut self,
        name: impl Into<String>,
        queue: Queue,
        uses: impl IntoIterator<Item = (ResourceId, Access)>,
    ) -> PassId {
        let id = PassId(self.passes.len());
        self.passes.push(Pass {
            name: name.into(),
            queue,
            uses: uses.into_iter().collect(),
            after: BTreeSet::new(),
        });
        id
    }
    /// Add a semantic ordering edge, for side effects not expressed as resources.
    pub fn after(&mut self, pass: PassId, dependency: PassId) -> Result<()> {
        ensure!(
            dependency.0 < self.passes.len() && pass.0 < self.passes.len(),
            "Unknown pass"
        );
        self.passes[pass.0].after.insert(dependency.0);
        Ok(())
    }
    /// Add shared bindless reachability sets before compilation. Conservative reads are legal.
    pub fn accesses(
        &mut self,
        pass: PassId,
        uses: impl IntoIterator<Item = (ResourceId, Access)>,
    ) -> Result<()> {
        self.passes
            .get_mut(pass.0)
            .ok_or_else(|| anyhow::anyhow!("Unknown graph pass"))?
            .uses
            .extend(uses);
        Ok(())
    }
    /// Order passes, reject missing producers/cycles, and emit exact ownership pairs.
    pub fn compile(&self) -> Result<CompiledGraph> {
        let count = self.passes.len();
        let mut dependencies = self
            .passes
            .iter()
            .map(|p| p.after.clone())
            .collect::<Vec<_>>();
        for pass in &self.passes {
            let mut seen = BTreeSet::new();
            for &(id, _) in &pass.uses {
                ensure!(id.0 < self.resources.len(), "Unknown graph resource");
                ensure!(
                    seen.insert(id),
                    "Declare one combined access per pass/resource"
                );
            }
        }
        for (r, resource) in self.resources.iter().enumerate() {
            let uses = self
                .passes
                .iter()
                .enumerate()
                .filter_map(|(p, pass)| {
                    pass.uses
                        .iter()
                        .find(|(id, _)| id.0 == r)
                        .map(|(_, access)| (p, *access))
                })
                .collect::<Vec<_>>();
            let writers = uses
                .iter()
                .filter(|(_, a)| a.writes())
                .map(|(p, _)| *p)
                .collect::<Vec<_>>();
            for pair in writers.windows(2) {
                dependencies[pair[1]].insert(pair[0]);
            }
            for (p, _) in uses.iter().copied().filter(|(_, a)| !a.writes()) {
                let previous = writers.iter().copied().take_while(|w| *w < p).last();
                let producer = if previous.is_none() && resource.state.access == Access::Undefined {
                    Some(
                        *writers
                            .first()
                            .ok_or_else(|| anyhow::anyhow!("Resource {r} read with no producer"))?,
                    )
                } else {
                    previous
                };
                if let Some(producer) = producer {
                    dependencies[p].insert(producer);
                }
                let next_writer = match producer {
                    Some(producer) => writers.iter().copied().find(|w| *w > producer),
                    None => writers.first().copied(),
                };
                if let Some(next) = next_writer {
                    dependencies[next].insert(p);
                }
            }
        }
        let mut ordered = Vec::with_capacity(count);
        let mut done = BTreeSet::new();
        while ordered.len() < count {
            let next = (0..count)
                .find(|p| !done.contains(p) && dependencies[*p].iter().all(|d| done.contains(d)));
            let Some(next) = next else {
                bail!("Render graph contains a dependency cycle");
            };
            done.insert(next);
            ordered.push(next);
        }
        let mut passes = ordered
            .iter()
            .map(|&p| CompiledPass {
                id: PassId(p),
                name: self.passes[p].name.clone(),
                queue: self.passes[p].queue,
                before: Vec::new(),
                after: Vec::new(),
                waits: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut states = self.resources.iter().map(|r| r.state).collect::<Vec<_>>();
        let mut last_pass = vec![None::<usize>; self.resources.len()];
        for (index, &original) in ordered.iter().enumerate() {
            let pass = &self.passes[original];
            for &(id, after) in &pass.uses {
                let before = states[id.0];
                let resource = self.resources[id.0].resource;
                if before.queue != pass.queue && before.access != Access::Undefined {
                    let previous = last_pass[id.0].ok_or_else(|| {
                        anyhow::anyhow!("Imported resource changes queue without a release pass")
                    })?;
                    passes[previous].after.push(Barrier {
                        resource,
                        before: before.access,
                        after,
                        handoff: Some(Handoff {
                            from: before.queue,
                            to: pass.queue,
                            release: true,
                        }),
                    });
                    passes[index].before.push(Barrier {
                        resource,
                        before: before.access,
                        after,
                        handoff: Some(Handoff {
                            from: before.queue,
                            to: pass.queue,
                            release: false,
                        }),
                    });
                    passes[index].waits.push(previous);
                } else if before.access != after || before.access.writes() || after.writes() {
                    passes[index].before.push(Barrier {
                        resource,
                        before: before.access,
                        after,
                        handoff: None,
                    });
                }
                states[id.0] = State {
                    access: after,
                    queue: pass.queue,
                };
                last_pass[id.0] = Some(index);
            }
            for &dependency in &dependencies[original] {
                if self.passes[dependency].queue != pass.queue {
                    let source = ordered
                        .iter()
                        .position(|p| *p == dependency)
                        .ok_or_else(|| anyhow::anyhow!("Missing scheduled dependency"))?;
                    passes[index].waits.push(source);
                }
            }
            passes[index].waits.sort_unstable();
            passes[index].waits.dedup();
        }
        Ok(CompiledGraph {
            passes,
            final_states: states,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buffer() -> Resource {
        let mut a = ara_gpu::arena::Arena::<ara_gpu::BufferKind, _>::default();
        Resource::Buffer(a.insert(()).unwrap())
    }
    #[test]
    fn sorts_producer_before_reader_and_pairs_ownership() {
        let mut g = Graph::default();
        let r = g.import(
            buffer(),
            State {
                access: Access::Undefined,
                queue: Queue::Compute,
            },
        );
        let read = g.pass("lighting", Queue::Graphics, [(r, Access::ShaderRead)]);
        let write = g.pass("cull", Queue::Compute, [(r, Access::Storage)]);
        let c = g.compile().unwrap();
        assert_eq!(c.passes[0].id, write);
        assert_eq!(c.passes[1].id, read);
        let release = c.passes[0].after[0];
        let acquire = c.passes[1].before[0];
        assert!(release.handoff.unwrap().release);
        assert!(!acquire.handoff.unwrap().release);
        assert_eq!(release.before, acquire.before);
        assert_eq!(release.after, acquire.after);
        assert_eq!(c.passes[1].waits, [0]);
    }
    #[test]
    fn rejects_cycles_and_uninitialized_reads() {
        let mut g = Graph::default();
        let r = g.import(
            buffer(),
            State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
        );
        g.pass("bad", Queue::Graphics, [(r, Access::ShaderRead)]);
        assert!(g.compile().is_err());
        let mut g = Graph::default();
        let a = g.pass("a", Queue::Graphics, []);
        let b = g.pass("b", Queue::Graphics, []);
        g.after(a, b).unwrap();
        g.after(b, a).unwrap();
        assert!(g.compile().is_err());
    }
    #[test]
    fn emits_same_layout_write_hazard_and_orders_war() {
        let mut g = Graph::default();
        let r = g.import(
            buffer(),
            State {
                access: Access::Storage,
                queue: Queue::Compute,
            },
        );
        g.pass("read", Queue::Compute, [(r, Access::ShaderRead)]);
        g.pass("write", Queue::Compute, [(r, Access::Storage)]);
        g.pass("write2", Queue::Compute, [(r, Access::Storage)]);
        let c = g.compile().unwrap();
        assert_eq!(c.passes[2].before.len(), 1);
        assert_eq!(c.passes[2].before[0].before, Access::Storage);
    }
}
