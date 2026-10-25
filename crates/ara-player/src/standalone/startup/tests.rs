use super::*;

#[test]
fn prepared_runtime_moves_to_owner_and_continues_its_same_ecs_physics_world() -> Result<()> {
    let scene: ara_play::PlayScene = serde_json::from_value(json!({
        "format":1,"scene":"sc_startup1","types":[],"seed":7,"tick_hz":60,
        "assets":{},"cache":"","nav_cache":null,
        "documents":[{"path":"main.scene","kind":"scene","id":"sc_startup1","records":{
            "e_body":{"Transform":{"position":[0,2,0]},
                "RigidBody":{"motion":"dynamic","mass":1},
                "Collider":{"kind":"sphere","radius":0.25}}
        }}]
    }))?;
    let worker = thread::spawn(move || {
        World::prepare_packed(
            &scene,
            Arc::new(ara_runtime::assets::MemoryAssets::default()),
        )
    });
    let (prepared, build_ms) = worker
        .join()
        .map_err(|_| anyhow::anyhow!("world worker panicked"))??;
    let mut world = prepared.map_runtime(|runtime| Ok(crate::net::Play::Local(runtime)))?;
    world.start(true, build_ms)?;
    world.runtime.step(6, &[])?;
    assert_eq!(world.runtime.tick(), 6);
    let height = world::read_path(
        world.runtime.play().session(),
        "e_body.Transform.position.y",
    )?;
    assert!(
        height
            .context("body position")?
            .as_f64()
            .context("body height")?
            < 2.
    );
    Ok(())
}
