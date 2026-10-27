//! Opt-in full-world diagnostic costs, measured without advancing simulation.
use super::*;

pub(super) fn hash_cost(world: &World) -> Result<Value> {
    let session = world.runtime.play().session();
    let mut entities = 0usize;
    let mut components = 0usize;
    let mut total_entities = 0usize;
    let mut total_components = 0usize;
    for entity in world.runtime.world().iter_entities() {
        total_entities += 1;
        total_components += entity.archetype().components().len();
        if entity.contains::<Identity>() {
            entities += 1;
            components += entity.archetype().components().len();
        }
    }
    let expected = session.state_hash()?;
    let mut times = Vec::new();
    for _ in 0..12 {
        let began = Instant::now();
        let hash = session.state_hash()?;
        times.push(began.elapsed().as_secs_f64() * 1000.);
        anyhow::ensure!(
            hash == expected,
            "state hash changed without a simulation boundary"
        );
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"sceneEntities":entities,"sceneEcsComponentInstances":components,
        "totalEcsEntitiesIncludingResources":total_entities,"totalEcsComponentInstances":total_components,
        "reflectedComponentTypes":session.schema().components().count(),"samples":times.len(),
        "meanMsPerHash":times.iter().sum::<f64>()/times.len() as f64,"medianMsPerHash":times[times.len()/2],
        "minMsPerHash":times[0],"maxMsPerHash":times[times.len()-1],
        "normalSessionHashesByDefault":false,"liveRecordingHashEveryTicks":30,"offlineReplayHashesEveryTick":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the downloaded Outpost project copied under target"]
    fn full_outpost_replays_its_first_boundaries() -> Result<()> {
        let root = PathBuf::from(
            std::env::var_os("ARA_OUTPOST_TEST_PROJECT").context("scratch Outpost project")?,
        );
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .canonicalize()?;
        anyhow::ensure!(
            root.canonicalize()?.starts_with(scratch),
            "fixture must be a scratch project under target"
        );
        let content = crate::game_app::content::project(&root, |_| Ok(()))?;
        let net = ara_play::NetStart {
            players: 4,
            clients: vec![None; 3],
            join: None,
        };
        let (mut world, _) = World::prepare_game(&content.scene, Some(&net), None)?;
        ara_runtime_net::attach_motors(&mut world.runtime);
        if let Some(path) = std::env::var_os("ARA_OUTPOST_TEST_REPLAY") {
            let replay = ara_sim::Replay::from_bytes(&std::fs::read(path)?)?;
            let first = world.runtime.replay(&replay)?;
            let second = world.runtime.replay(&replay)?;
            anyhow::ensure!(
                first == second,
                "full recording must replay identically twice"
            );
            println!("verified {} boundaries twice: {first}", replay.ticks.len());
            return Ok(());
        }
        world
            .runtime
            .play_mut()
            .session_mut()
            .set_hash_interval(std::num::NonZeroU32::new(30).expect("nonzero cadence"))?;
        world.runtime.play_mut().record()?;
        world.runtime.step(30, &[])?;
        let live = serde_json::to_value(world.runtime.play().snapshot()?)?;
        let replay = world.runtime.play_mut().finish_recording()?;
        anyhow::ensure!(
            replay.ticks.len() == 30,
            "fixture must execute 30 real boundaries"
        );
        let result = world.runtime.replay(&replay);
        let restored = serde_json::to_value(world.runtime.play().snapshot()?)?;
        let mut differences = Vec::new();
        diff("", &live, &restored, &mut differences);
        let scratch = root.join("replay-differences.json");
        std::fs::write(&scratch, serde_json::to_vec_pretty(&differences)?)?;
        result.with_context(|| format!("snapshot differences: {}", scratch.display()))?;
        Ok(())
    }

    fn diff(path: &str, a: &Value, b: &Value, output: &mut Vec<Value>) {
        if a == b || output.len() >= 100 {
            return;
        }
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                for key in a
                    .keys()
                    .chain(b.keys())
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    diff(
                        &format!("{path}/{key}"),
                        a.get(key).unwrap_or(&Value::Null),
                        b.get(key).unwrap_or(&Value::Null),
                        output,
                    );
                }
            }
            (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    diff(&format!("{path}/{i}"), a, b, output);
                }
            }
            _ => output.push(json!({"path":path,"live":a,"replay":b})),
        }
    }
}
