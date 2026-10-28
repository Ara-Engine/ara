//! Persist one real ECS entity and system across live body edits.
use anyhow::{Context, Result, ensure};
use ara_components::Transform;
use ara_sim::{
    EditWorld, PlayControl, Session, SessionAdapter, SessionMode, SimRegistry, StateHash,
    TickConfig,
};
use bevy_ecs::prelude::{Entity, Query, With, World};
use std::{path::PathBuf, time::Duration};

fn advance(mut transforms: Query<&mut Transform>) {
    for mut transform in &mut transforms {
        transform.position[0] += 1.;
    }
}

fn main() -> Result<()> {
    // SAFETY: Process entry, before any threads or ECS initialization.
    let arguments = unsafe { ara_hotpatch::args_os()? };
    ensure!(
        arguments.len() == 3 && arguments[1] == "--output",
        "use --output <scratch>"
    );
    let output = PathBuf::from(&arguments[2]);
    std::fs::create_dir_all(&output)?;
    let mut patches = ara_hotpatch::Hotpatch::connect();
    let mut schema = ara_schema::SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_components::register(&mut schema);
    let read = ara_doc::read(
        r#"{"$schema":"ara://schema/scene/1","id":"sc_12345678","records":{"e_0001":{"name":"Hotpatch","order":"a0","Transform":{}}}}"#,
        "probe.scene",
        &schema,
        None,
    );
    ensure!(
        !read.diags.iter().any(|d| d.is_error()),
        "invalid probe scene: {:?}",
        read.diags
    );
    let edit = EditWorld::new(
        SimRegistry::new(schema)?,
        read.doc.context("missing probe scene")?,
    )?;
    let mut session = Session::local(
        edit,
        TickConfig::default(),
        7,
        ara_input::ActionMap::default(),
    )?;
    session.add_system("advance", advance)?;
    let entity = session
        .world_mut()
        .query_filtered::<Entity, With<Transform>>()
        .single(session.world())?;
    let mut play = PlayControl::new(session, StateHash::of_bytes(b"Rust patch proof"));
    play.start();
    let checkpoint = play.snapshot()?;
    play.record()?;
    let mut guards_verified = false;
    loop {
        let applied = patches.frame_boundary();
        let before = play
            .session()
            .world()
            .get::<Transform>(entity)
            .context("entity lost")?
            .position[0];
        play.step(1, &[])?;
        let after = play
            .session()
            .world()
            .get::<Transform>(entity)
            .context("entity replaced")?
            .position[0];
        if applied && !guards_verified {
            let replay = ara_sim::Replay::from_bytes(&play.finish_recording()?.to_bytes()?)?;
            ensure!(
                replay.rust_hotpatched,
                "replay session header omitted Rust patch flag"
            );
            ensure!(
                replay.format == 2,
                "legacy readers could accept a patched replay"
            );
            let snapshot = serde_json::to_value(play.snapshot()?)?;
            ensure!(
                snapshot["format"] == 2 && snapshot["rust_hotpatched"] == true,
                "legacy readers could accept a patched snapshot"
            );
            ensure!(play.replay(&replay).is_err(), "patched replay was accepted");
            ensure!(
                play.restore(&checkpoint).is_err(),
                "patched restore was accepted"
            );
            let tick = play.session().tick();
            ensure!(
                play.step_with_adapter(&mut Network).is_err(),
                "patched network session was accepted"
            );
            ensure!(
                play.session().tick() == tick,
                "network refusal advanced the simulation"
            );
            ensure!(
                play.session()
                    .world()
                    .get::<Transform>(entity)
                    .context("determinism refusal lost the entity")?
                    .position[0]
                    == after,
                "determinism refusal mutated ECS state"
            );
            guards_verified = true;
        }
        let status = serde_json::json!({
            "pid":std::process::id(), "frame":play.session().tick(), "position":after,
            "delta":after-before, "rust":patches.status(),
            "rust_hotpatched":play.session().rust_hotpatched(), "guards_verified":guards_verified,
            "entity":entity.to_bits(),
        });
        std::fs::write(output.join("status.json"), serde_json::to_vec(&status)?)?;
        if applied {
            println!("ARA_SIM_PATCH_TICK {status}");
        }
        if output.join("stop").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

struct Network;
impl SessionAdapter for Network {
    fn mode(&self) -> SessionMode {
        SessionMode::Listen
    }
    fn before_tick(&mut self, _: &mut Session) -> std::result::Result<(), ara_sim::SimError> {
        Ok(())
    }
    fn after_tick(
        &mut self,
        _: &World,
        _: &ara_sim::TickRecord,
    ) -> std::result::Result<(), ara_sim::SimError> {
        Ok(())
    }
}
