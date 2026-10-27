#![allow(clippy::unwrap_used)]
mod support;
use ara_input::{Injection, PlayerId};
use ara_net::*;
use ara_sim::SessionMode;
use std::time::{Duration, Instant};
use support::*;

#[test]
fn late_device_input_reaches_an_already_running_listen_authority() -> Result<()> {
    let mut server = Server::<Movement>::new(
        session()?,
        policies()?,
        ServerConfig {
            mode: SessionMode::Listen,
            local_owner: PlayerId(0),
            start_clients: 0,
            ..Default::default()
        },
    )?;
    configure(&mut server)?;
    server.record(build(), vec![])?;
    let start = Instant::now();
    while server.session().tick() < 30 {
        assert!(start.elapsed() < Duration::from_secs(10));
        server.poll()?;
        std::thread::sleep(Duration::from_millis(2));
    }
    let endpoint = server.address()?;
    let token = server.token(PlayerId(1), endpoint)?;
    let mut client = Client::<Movement>::new(
        policies()?,
        ClientConfig {
            player: PlayerId(1),
            endpoint,
            hz: 60,
            tick_limit: None,
        },
        token,
    )?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        server.poll()?;
        client.poll()?;
        let status = client.status();
        if status.synced {
            client.inject(
                status.predicted_tick + 1,
                Injection {
                    player: PlayerId(1),
                    action: "Move".into(),
                    value: script(status.predicted_tick + 1, PlayerId(1)).unwrap(),
                    hold_ticks: 12,
                },
            )?;
        }
        let entity = server.session().entity("player1").unwrap();
        if server.session().world().get::<Motor>(entity).unwrap().x > 500 && status.checkpoints > 0
        {
            assert!(
                status.predicted_tick < server.session().tick() + 120,
                "prediction escaped authority: {status:?}"
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let entity = server.session().entity("player1").unwrap();
    panic!(
        "device input never reached authority: {:?}, {:?}",
        server.session().world().get::<Motor>(entity),
        client.status()
    );
}

#[test]
fn replica_reads_preserve_absent_components() -> Result<()> {
    let mut server = Server::<Movement>::new(
        session()?,
        policies()?,
        ServerConfig {
            start_clients: 1,
            ..Default::default()
        },
    )?;
    configure(&mut server)?;
    let endpoint = server.address()?;
    let token = server.token(PlayerId(1), endpoint)?;
    let mut client = Client::<Movement>::new(
        policies()?,
        ClientConfig {
            player: PlayerId(1),
            endpoint,
            hz: 60,
            tick_limit: None,
        },
        token,
    )?;
    let began = Instant::now();
    for _ in 0..600 {
        server.advance(Duration::from_millis(10))?;
        client.advance(Duration::from_millis(10))?;
        if let Some(item) = client.spawns().into_iter().find(|s| s.identity == "e_0003")
            && client.component(item.id, "Item")?.is_some()
        {
            assert_eq!(
                client.component(item.id, "Inventory")?,
                None,
                "a pickup must not acquire a phantom player inventory"
            );
            assert_eq!(client.sample_component(item.id, "Inventory", 0.5)?, None);
            return Ok(());
        }
    }
    panic!("pickup baseline missing after {:?}", began.elapsed());
}

#[test]
#[ignore = "focused projection-read profiling; run explicitly before/after hot-path changes"]
fn profile_replica_component_reads() -> Result<()> {
    let mut sim = session()?;
    for index in 0..600 {
        sim.world_mut().spawn((
            ara_sim::Identity(format!("e_load{index:03}")),
            Item {
                x: index as f32,
                available: true,
            },
        ));
    }
    let mut server = Server::<Movement>::new(
        sim,
        policies()?,
        ServerConfig {
            start_clients: 1,
            ..Default::default()
        },
    )?;
    configure(&mut server)?;
    let endpoint = server.address()?;
    let token = server.token(PlayerId(1), endpoint)?;
    let mut client = Client::<Movement>::new(
        policies()?,
        ClientConfig {
            player: PlayerId(1),
            endpoint,
            hz: 60,
            tick_limit: None,
        },
        token,
    )?;
    for _ in 0..2000 {
        server.advance(Duration::from_millis(10))?;
        client.advance(Duration::from_millis(10))?;
        if let Some(item) = client
            .spawns()
            .into_iter()
            .find(|s| s.identity == "e_load599")
            && client.component(item.id, "Item")?.is_some()
        {
            let began = Instant::now();
            for _ in 0..20_000 {
                std::hint::black_box(client.component(item.id, "Inventory")?);
            }
            println!(
                "component_read_us={} reads=20000 entities={}",
                began.elapsed().as_micros(),
                client.spawns().len()
            );
            return Ok(());
        }
    }
    panic!("profiling baseline missing");
}

#[test]
#[ignore = "focused authority projection profiling; run explicitly before/after hot-path changes"]
fn profile_authority_projection() -> Result<()> {
    let _initialized = session()?;
    let mut db = schema();
    let mut declarations = serde_json::json!({"$schema":"ara://schema/types/1","types":{}});
    for index in 0..100 {
        declarations["types"][format!("Private{index}")] = serde_json::json!({"kind":"component","doc":"Authority-only metadata.","fields":{"value":{"type":"u64","doc":"Private value."}}});
    }
    assert!(db.add_json_types(&declarations).is_empty());
    let mut registry = ara_sim::SimRegistry::new(db)?;
    registry.register_component::<Motor>()?;
    registry.register_component::<Item>()?;
    let mut sim = ara_sim::Session::local(
        ara_sim::EditWorld::new(
            registry,
            ara_doc::Document::new(
                "scene",
                ara_doc::AssetId::parse("sc_projectionbench").unwrap(),
            ),
        )?,
        Default::default(),
        1,
        Default::default(),
    )?;
    for index in 0..650 {
        sim.world_mut().spawn((
            ara_sim::Identity(format!("e_load{index:03}")),
            Item {
                x: index as f32,
                available: true,
            },
        ));
    }
    let policies = PolicyRegistry::new(sim.schema().clone())?;
    let mut server = Server::<Movement>::new(
        sim,
        policies,
        ServerConfig {
            mode: SessionMode::Listen,
            start_clients: 0,
            ..Default::default()
        },
    )?;
    for _ in 0..121 {
        server.advance(Duration::from_secs_f64(1. / 60.))?;
    }
    let began = Instant::now();
    for _ in 0..120 {
        server.advance(Duration::from_secs_f64(1. / 60.))?;
    }
    println!(
        "authority_projection_us={} ticks=120 entities=650 private_types=100",
        began.elapsed().as_micros()
    );
    Ok(())
}
