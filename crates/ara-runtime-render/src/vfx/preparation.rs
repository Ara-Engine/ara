use super::*;
pub(super) struct Pending {
    name: String,
    size: [u32; 2],
    pub warmup: ara_vfx_gpu::PipelineWarmup,
}
impl Effects {
    /// Discover and adopt live effects without compiling on the frame owner.
    pub(super) fn prepare_systems(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
    ) -> Result<()> {
        let size = renderer.render_size();
        let matches = |id: u64, name: &str, extent| {
            extent == size
                && frame
                    .effects
                    .iter()
                    .any(|e| e.entity == id && e.effect == name)
        };
        let stale: Vec<_> = self
            .live
            .iter()
            .filter(|(id, live)| !matches(**id, &live.name, live.size))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(live) = self.live.remove(&id) {
                live.system.retire(renderer.gpu_mut())?;
            }
        }
        let stale: Vec<_> = self
            .pending
            .iter()
            .filter(|(id, pending)| !matches(**id, &pending.name, pending.size))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(pending) = self.pending.remove(&id) {
                pending.warmup.cancel(renderer.gpu_mut())?;
            }
        }
        for effect in &frame.effects {
            if self.live.contains_key(&effect.entity)
                || self.failed.get(&effect.entity) == Some(&effect.effect)
            {
                continue;
            }
            let result = (|| {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    self.pending.entry(effect.entity)
                {
                    let definition = library(&effect.effect)?;
                    entry.insert(Pending {
                        name: effect.effect.to_owned(),
                        size,
                        warmup: ara_vfx_gpu::PipelineWarmup::new(
                            &cooked_root().join(&definition.name),
                        )?,
                    });
                }
                let pending = self
                    .pending
                    .get_mut(&effect.entity)
                    .context("effect warmup inserted")?;
                let Some(pipelines) = pending.warmup.poll(renderer.gpu_mut())? else {
                    return Ok(());
                };
                self.pending.remove(&effect.entity);
                let definition = library(&effect.effect)?;
                let system = GpuSystem::new_prepared(
                    renderer.gpu_mut(),
                    &definition,
                    &cooked_root().join(&definition.name),
                    size,
                    pipelines,
                )?;
                self.live.insert(
                    effect.entity,
                    Live {
                        name: effect.effect.to_owned(),
                        size,
                        system,
                        tick: frame.tick.saturating_sub(1),
                        rendered: false,
                        warm_remaining: if renderer.is_native() { 150 } else { 0 },
                    },
                );
                Ok::<_, anyhow::Error>(())
            })();
            if let Err(error) = result {
                if let Some(pending) = self.pending.remove(&effect.entity) {
                    pending.warmup.cancel(renderer.gpu_mut())?;
                }
                eprintln!("ARA7303: {error:#}");
                self.failed.insert(effect.entity, effect.effect.to_owned());
            }
        }
        Ok(())
    }
    /// Explicit capture boundary: wait for all requested effects' compiled PSOs.
    pub(crate) fn wait_pipelines(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
    ) -> Result<()> {
        loop {
            self.prepare_systems(renderer, frame)?;
            if self.pending.is_empty() {
                anyhow::ensure!(
                    !frame
                        .effects
                        .iter()
                        .any(|e| self.failed.get(&e.entity) == Some(&e.effect)),
                    "requested capture effect could not prepare its cooked pipelines"
                );
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
