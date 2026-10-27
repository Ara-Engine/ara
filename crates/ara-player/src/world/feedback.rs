//! Reliable replicated counters drive transient presentation without changing the world.
use super::*;
use ara_runtime::sync::WorldPose;
use ara_sim::Identity;

#[derive(Default)]
pub(super) struct Feedback {
    source: Option<ara_play::GameUi>,
    seen: BTreeMap<(String, usize), u64>,
    clips: BTreeMap<usize, ara_audio::Clip>,
    tick: Option<u64>,
}

impl Feedback {
    pub(super) fn new(source: Option<&ara_play::GameUi>) -> Self {
        Self {
            source: source.cloned(),
            ..Self::default()
        }
    }
    pub(super) fn update(
        &mut self,
        presenter: &mut AudioPresenter,
        world: &ara_sim::bevy_ecs::world::World,
        listener: &ara_audio::AudioListener,
        assets: &dyn ara_runtime::assets::AssetSource,
        tick: u64,
    ) -> Result<()> {
        if self.tick == Some(tick) {
            return Ok(());
        }
        self.tick = Some(tick);
        let Some(ui) = &self.source else {
            return Ok(());
        };
        for entity in world.iter_entities() {
            let Some(id) = entity.get::<Identity>() else {
                continue;
            };
            for music in ui.music.iter().filter(|m| m.entity == id.0) {
                let value = ara_runtime_script::read(world, entity.id(), &music.component)
                    .and_then(|v| v.get(&music.field).and_then(Value::as_f64));
                presenter.set_music_intensity(&music.target, value.map(|v| v as f32));
            }
            for (index, route) in ui.sounds.iter().enumerate() {
                let Some(counter) = ara_runtime_script::read(world, entity.id(), &route.component)
                    .and_then(|v| v.get(&route.field).and_then(Value::as_u64))
                else {
                    continue;
                };
                let previous = self.seen.insert((id.0.to_owned(), index), counter);
                if counter == 0 || previous.is_none_or(|last| last == counter) {
                    continue;
                }
                let position = entity
                    .get::<WorldPose>()
                    .map_or([0.; 3], |p| p.0.translation.to_array());
                let distance = position
                    .into_iter()
                    .zip(listener.position)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt();
                let gain = route.gain / (1. + distance * distance / 64.);
                if gain < 0.005 {
                    continue;
                }
                if let std::collections::btree_map::Entry::Vacant(entry) = self.clips.entry(index) {
                    entry.insert(ara_runtime_audio::clip(assets, &route.clip)?);
                }
                let clip = self.clips.get(&index).context("feedback clip loaded")?;
                let engine = presenter.engine();
                let bus = engine
                    .bus("sfx")
                    .or_else(|| engine.bus("master"))
                    .context("audio master bus")?;
                let playback = ara_audio::Playback {
                    at_frame: engine.next_frame(),
                    spatial: false,
                    parameters: ara_audio::VoiceParameters {
                        gain,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                // A Clip is an immutable PCM handle; events share its sample allocation.
                engine.play(bus, clip.clone(), playback)?;
            }
        }
        Ok(())
    }
}

pub(super) fn effects(
    ui: &ara_play::GameUi,
    runtime: &ara_runtime::Runtime,
    frame: &mut RenderFrame,
) {
    for entity in runtime.world().iter_entities() {
        let Some(id) = entity.get::<Identity>() else {
            continue;
        };
        for route in &ui.sounds {
            let Some(tick) =
                ara_runtime_script::read(runtime.world(), entity.id(), &route.component)
                    .and_then(|v| v.get(&route.field).and_then(Value::as_u64))
            else {
                continue;
            };
            if route.hide_source && tick > 0 {
                frame.draws.retain(|draw| draw.address != id.0);
            }
            let Some(effect) = &route.effect else {
                continue;
            };
            if tick == 0 || frame.tick.saturating_sub(tick) > 24 {
                continue;
            }
            let position = entity
                .get::<WorldPose>()
                .map_or([0.; 3], |p| p.0.translation.to_array());
            frame.effects.push(ara_runtime::extract::EffectView {
                entity: ara_ui::source::stable_id(&format!("feedback:{}:{}", id.0, route.field)),
                label: format!("{} feedback", id.0),
                effect: effect.to_owned(),
                position,
                scale: 0.4,
                intensity: 0.8,
            });
        }
    }
}

pub(super) fn animation(ui: &ara_play::GameUi, runtime: &mut ara_runtime::Runtime) {
    let tick = runtime.tick();
    let world = runtime.play_mut().session_mut().world_mut();
    let mut inputs = Vec::new();
    for entity in world.iter_entities() {
        for route in &ui.sounds {
            let Some(parameter) = &route.animation else {
                continue;
            };
            if let Some(counter) = ara_runtime_script::read(world, entity.id(), &route.component)
                .and_then(|v| v.get(&route.field).and_then(Value::as_u64))
            {
                inputs.push((
                    entity.id(),
                    parameter.as_str(),
                    counter > 0 && tick.saturating_sub(counter) < 24,
                ));
            }
        }
    }
    for (entity, name, value) in inputs {
        if let Some(mut slot) = world.get_mut::<ara_runtime::animation::AnimatorSlot>(entity)
            && let Some(parameter) = slot.program.controller.parameter(name)
            && let Err(error) = slot
                .animator
                .set(parameter, ara_anim::graph::ParameterValue::Bool(value))
        {
            slot.fault = Some(error.0);
        }
    }
}
