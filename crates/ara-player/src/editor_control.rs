//! Tick-boundary possession acknowledgements share the existing nonblocking player loop.
use super::*;
impl Player {
    pub(super) fn control_request(&mut self, id: Value, params: Value) -> Result<()> {
        let outcome = (|| -> std::result::Result<Option<Value>, Failure> {
            let control: ara_play::Control = parse(params)?;
            let world = self.world.as_mut().ok_or_else(no_world)?;
            let pending = world.transfer_control(id.clone(), control)?;
            if world.runtime.networked() || world.runtime.running() {
                world.pending_controls.push(pending);
                Ok(None)
            } else {
                world.runtime.step(1, &[])?;
                world.mark();
                world.adopted_control(&pending)?;
                Ok(Some(serde_json::to_value(pending.result)?))
            }
        })();
        match outcome {
            Ok(None) => {}
            Ok(Some(value)) => self.link.respond(id, Ok(value))?,
            Err(Failure::Params(problem)) => self
                .link
                .respond(id, Err(error(INVALID_PARAMS, problem, Value::Null)))?,
            Err(Failure::Engine(problem)) => self.link.respond(
                id,
                Err(error(ENGINE_ERROR, format!("{problem:#}"), Value::Null)),
            )?,
            Err(Failure::Unknown) => {}
        }
        self.link.flush()?;
        Ok(())
    }
    pub(super) fn acknowledge_controls(&mut self) -> Result<()> {
        let Some(world) = self.world.as_mut() else {
            return Ok(());
        };
        let tick = world.runtime.tick();
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut world.pending_controls)
            .into_iter()
            .partition(|control| control.tick <= tick);
        world.pending_controls = waiting;
        for control in ready {
            let result = match world.adopted_control(&control) {
                Ok(()) => serde_json::to_value(control.result).map_err(anyhow::Error::from),
                Err(problem) => Err(problem),
            };
            self.link.respond(
                control.id,
                result.map_err(|problem| error(ENGINE_ERROR, format!("{problem:#}"), Value::Null)),
            )?;
        }
        self.link.flush()?;
        Ok(())
    }
}
