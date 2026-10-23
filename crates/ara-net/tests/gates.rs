#![allow(clippy::unwrap_used)]
mod support;
use ara_input::{Injection, PlayerId};
use ara_net::*;
use ara_schema::Audience;
use ara_sim::SessionMode;
use serde_json::json;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use support::*;

#[test]
fn spawn_baselines_preserve_projection_ownership() -> Result<()> {
    let mut registry = ara_sim::SimRegistry::new(schema())?;
    registry.register_component::<Item>()?;
    let scene = ara_doc::read(
        r#"{"$schema":"ara://schema/scene/1","id":"sc_baselines","records":{"e_projected":{"Item":{"x":0.0,"available":true}}}}"#,
        "scene.scene", registry.schema(), None,
    ).doc.expect("projected baseline fixture");
    let mut sim = ara_sim::Session::local(
        ara_sim::EditWorld::new(registry, scene)?,
        ara_sim::TickConfig::default(),
        1,
        Default::default(),
    )?;
    sim.world_mut().spawn((
        ara_sim::Identity("e_runtime".into()),
        Item {
            x: 2.0,
            available: true,
        },
    ));
    let mut server = Server::<Movement>::new(
        sim,
        policies()?,
        ServerConfig {
            tick_limit: Some(1),
            ..Default::default()
        },
    )?;
    let endpoint = server.address()?;
    let mut client = Client::<Movement>::new(
        policies()?,
        ClientConfig {
            player: PlayerId(0),
            endpoint,
            hz: 60,
            tick_limit: Some(1),
        },
        server.token(PlayerId(0), endpoint)?,
    )?;
    let start = Instant::now();
    loop {
        server.poll()?;
        client.poll()?;
        let spawns = client.spawns();
        if spawns.len() == 2 {
            for spawn in spawns {
                assert_eq!(spawn.projected, spawn.identity == "e_projected");
            }
            return Ok(());
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "spawn baselines did not arrive"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn local_has_zero_serialization_or_network_resources() -> Result<()> {
    use std::sync::atomic::Ordering;
    let mut sim = session()?;
    sim.world_mut()
        .spawn((ara_sim::Identity("probe".into()), Probe { value: 1 }));
    // Bevy installs its empty schedule registry on the first tick, even offline.
    let mut baseline = session()?;
    baseline.step()?;
    let resources = baseline.world().iter_resources().count();
    SERIALIZATIONS.store(0, Ordering::Relaxed);
    sim.submit_typed(
        1,
        ara_sim::CommandContext {
            player: PlayerId(0),
            sequence: 1,
            target: Some("player0".into()),
        },
        Heal { count: 1 },
    )?;
    for _ in 0..60 {
        sim.step()?;
    }
    assert_eq!(SERIALIZATIONS.load(Ordering::Relaxed), 0);
    assert_eq!(sim.world().iter_resources().count(), resources);
    assert_eq!(resources, baseline.world().iter_resources().count());
    assert_eq!(sim.mode(), SessionMode::Local);
    Ok(())
}

#[test]
fn host_registers_six_scoped_methods_and_bots_use_actions() -> Result<()> {
    use ara_host::{Host, HostConfig, Project};
    use ara_proto::{ConnId, Scope};
    let dir = std::env::temp_dir().join(format!("ara-net-host-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("ara.project.json"),
        ara_json::to_canonical_string(&json!({"name":"net-gate"})),
    )?;
    let mut host = Host::open(
        Project::open(&dir).map_err(|e| NetError(e.to_string()))?,
        HostConfig::default(),
    )
    .map_err(|e| NetError(e.to_string()))?;
    register(
        &mut host,
        WorldControl::<Movement>::new(policies()?, session, configure),
    )
    .map_err(|e| NetError(e.to_string()))?;
    let methods: Vec<_> = host
        .catalog()
        .iter()
        .filter(|m| m.service == "net")
        .collect();
    assert_eq!(methods.len(), 6);
    assert!(
        methods
            .iter()
            .all(|m| m.scope == Some(Scope::Net) && m.tool.as_ref().unwrap().tool == "ara_net")
    );
    let agent = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"net-agent"}),
        )
        .unwrap();
    assert!(
        host.call(
            ConnId::LOCAL,
            "net.status",
            json!({"session":agent["session"]})
        )
        .is_err()
    );
    let human = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"editor","author":"human"}),
        )
        .unwrap();
    let s = human["session"].as_str().unwrap();
    let status = host
        .call(ConnId::LOCAL, "net.status", json!({"session":s}))
        .unwrap();
    assert_eq!(status["running"], false);
    host.call(
        ConnId::LOCAL,
        "net.start",
        json!({"session":s,"mode":"dedicated","clients":3,"seed":21668}),
    )
    .unwrap();
    host.call(ConnId::LOCAL,"net.bots",json!({"session":s,"player":0,"tick":100,"action":"Move","value":{"Axis2":[1.0,0.0]},"holdTicks":10})).unwrap();
    let conditions = serde_json::to_value(Conditions::ADVERSE)?;
    host.call(
        ConnId::LOCAL,
        "net.conditions",
        json!({"session":s,"player":0,"up":conditions,"down":conditions}),
    )
    .unwrap();
    host.call(ConnId::LOCAL, "net.profile", json!({"session":s}))
        .unwrap();
    host.call(ConnId::LOCAL, "net.stop", json!({"session":s}))
        .unwrap();
    Ok(())
}

#[test]
fn schema_privacy_quantization_intent_replay_and_lifecycle() -> Result<()> {
    let policies = policies()?;
    let value = json!({"hp":50,"items":65535,"secret":3735928559u32});
    let public = policies.project("Inventory", &value, Audience::Relevant)?;
    let private = policies.project("Inventory", &value, Audience::Owner)?;
    assert_eq!(policies.component(&public, "Inventory")?, json!({"hp":50}));
    assert_eq!(
        policies.component(&private, "Inventory")?,
        json!({"items":65535})
    );
    let a = policies.project(
        "Item",
        &json!({"x":1.234,"available":true}),
        Audience::Relevant,
    )?;
    let b = policies.project(
        "Item",
        &json!({"x":2.234,"available":false}),
        Audience::Relevant,
    )?;
    assert!((policies.component(&a, "Item")?["x"].as_f64().unwrap() - 1.23).abs() < 1e-6);
    let interpolated = policies.component(&policies.interpolate(&a, &b, 0.5), "Item")?;
    assert!((interpolated["x"].as_f64().unwrap() - 1.73).abs() < 1e-6);
    assert_eq!(interpolated["available"], false);
    let mut sim = session()?;
    let mut gate = IntentGate::new(schema());
    gate.expose(Expose {
        command: "heal".into(),
        params: "Heal".into(),
        count: 1,
        window_ticks: 60,
    })?;
    let intent = Intent {
        tick: 1,
        sequence: 1,
        command: "heal".into(),
        target: "player0".into(),
        payload: serde_json::to_vec(&json!({"count":1}))?,
    };
    assert!(
        gate.submit(&mut sim, PlayerId(0), Some(PlayerId(0)), intent.clone())
            .is_err()
    );
    gate.authenticate(PlayerId(0));
    assert!(
        gate.submit(&mut sim, PlayerId(0), Some(PlayerId(1)), intent.clone())
            .is_err()
    );
    gate.submit(&mut sim, PlayerId(0), Some(PlayerId(0)), intent.clone())?;
    assert!(
        gate.submit(&mut sim, PlayerId(0), Some(PlayerId(0)), intent.clone())
            .is_err()
    );
    assert!(
        gate.submit(
            &mut sim,
            PlayerId(0),
            Some(PlayerId(0)),
            Intent {
                sequence: 2,
                ..intent.clone()
            }
        )
        .is_err()
    );
    assert!(
        gate.submit(
            &mut sim,
            PlayerId(0),
            Some(PlayerId(0)),
            Intent {
                sequence: 3,
                payload: serde_json::to_vec(&json!({"count":99}))?,
                ..intent.clone()
            }
        )
        .is_err()
    );
    assert!(
        gate.submit(
            &mut sim,
            PlayerId(0),
            Some(PlayerId(0)),
            Intent {
                sequence: 4,
                command: "cmd.apply".into(),
                ..intent.clone()
            }
        )
        .is_err()
    );
    assert!(
        gate.submit(
            &mut sim,
            PlayerId(0),
            Some(PlayerId(0)),
            Intent {
                sequence: 5,
                tick: 10000,
                ..intent
            }
        )
        .is_err()
    );
    // Begin before queued work: snapshot restore must not duplicate pending commands.
    let mut trace = Trace::begin(&sim, build(), vec![21668])?;
    let record = sim.step()?;
    trace.push(&sim, record)?;
    let mut replay = session()?;
    assert_eq!(trace.replay(&mut replay)?, None);
    trace.states[0]["entities"][0]["components"]["Door"]["open"] = json!(false);
    let divergence = trace.replay(&mut replay)?.unwrap();
    assert_eq!(divergence.tick, 1);
    assert!(
        divergence.field.contains("Door.open"),
        "{}",
        divergence.field
    );
    let mut ids = WorldIds::default();
    let old = ids.spawn("one")?;
    ids.despawn("one");
    let new = ids.spawn("two")?;
    assert_eq!(old.slot, new.slot);
    assert!(new.epoch > old.epoch);
    let mut refs = DeferredReferences::new(2);
    assert!(!refs.resolve(new, old)?);
    assert_eq!(refs.spawned(old)?, vec![new]);
    refs.despawned(old);
    assert!(refs.spawned(old).is_err());
    let mut grid = Interest::new(10.0)?;
    let spawn = Spawn {
        id: new,
        identity: "two".into(),
        projected: false,
        prefab: "@a.prefab#e_0001".into(),
        version: [0; 32],
        owner: Some(PlayerId(2)),
        team: Some(1),
    };
    grid.update(&spawn, [100.0, 100.0])?;
    let view = ConnectionView {
        player: PlayerId(2),
        position: [0.0; 2],
        radius: 1.0,
        team: None,
        team_only: true,
        bytes_per_second: 1200,
    };
    assert!(grid.visible(&view)?.contains(&new));
    assert!(
        !grid
            .visible(&ConnectionView {
                player: PlayerId(3),
                ..view
            })?
            .contains(&new)
    );
    let key = ara_sim::PredictionKey {
        player: PlayerId(1),
        tick: 2,
        sequence: 1,
    };
    let mut cues = CueLedger::new(2);
    assert!(cues.predict(key)?);
    assert!(!cues.predict(key)?);
    assert!(cues.confirm(key)?);
    assert!(!cues.confirm(key)?);
    cues.retire(2);
    assert!(!cues.predict(key)?);
    let retracted = ara_sim::PredictionKey {
        tick: 3,
        sequence: 2,
        ..key
    };
    assert!(cues.predict(retracted)?);
    assert!(cues.retract(retracted));
    assert!(!cues.predict(retracted)?);
    let too_large = Intent {
        tick: 1,
        sequence: 1,
        command: "heal".into(),
        target: "player0".into(),
        payload: vec![0; 4097],
    };
    assert!(postcard::from_bytes::<Intent>(&postcard::to_allocvec(&too_large)?).is_err());
    Ok(())
}

#[test]
fn nested_privacy_and_quantized_rotation_interpolation() -> Result<()> {
    let mut db = schema();
    assert!(db.add_json_types(&json!({"$schema":"ara://schema/types/1","types":{
        "NestedStats":{"kind":"struct","doc":"A mixed visibility value.","fields":{
            "hp":{"type":"u16","doc":"Public health.","replicate":{"audience":"relevant"}},
            "pin":{"type":"u16","doc":"Authority-only secret."}}},
        "StatsEnvelope":{"kind":"component","doc":"A nested replicated value.","fields":{
            "stats":{"type":"NestedStats","doc":"Visible stats group.","replicate":{"audience":"relevant"}}}},
        "Turn":{"kind":"component","doc":"Rotation policies.","fields":{
            "q":{"type":"quat","doc":"Quaternion orientation.","replicate":{"audience":"relevant","quantize":0.001,"interpolate":"slerp"}},
            "euler":{"type":"vec3","doc":"XYZ Euler degrees.","unit":"deg","replicate":{"audience":"relevant","interpolate":"slerp"}}}}
    }})).is_empty());
    let policies = PolicyRegistry::new(db)?;
    let nested = policies.project(
        "StatsEnvelope",
        &json!({"stats":{"hp":50,"pin":999}}),
        Audience::Relevant,
    )?;
    assert_eq!(
        policies.component(&nested, "StatsEnvelope")?,
        json!({"stats":{"hp":50}})
    );
    let a = policies.project(
        "Turn",
        &json!({"q":[0,0,0,1],"euler":[0,0,170]}),
        Audience::Relevant,
    )?;
    let b = policies.project(
        "Turn",
        &json!({"q":[0,0,1,0],"euler":[0,0,-170]}),
        Audience::Relevant,
    )?;
    let midpoint = policies.component(&policies.interpolate(&a, &b, 0.5), "Turn")?;
    assert!((midpoint["q"][2].as_f64().unwrap() - 0.5_f64.sqrt()).abs() < 1e-6);
    assert!((midpoint["euler"][2].as_f64().unwrap().abs() - 180.0).abs() < 1e-3);
    Ok(())
}

#[test]
fn datagram_conditions_duplicate_reorder_drop_and_shape() -> Result<()> {
    use std::net::UdpSocket;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    receiver.set_nonblocking(true)?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let mut link = Link::new(
        21668,
        Conditions {
            latency_ms: 10,
            duplication: 10000,
            reordering: 10000,
            bandwidth: 1000,
            ..Default::default()
        },
    )?;
    assert_eq!(link.seed(), 21668);
    link.set_server(receiver.local_addr()?);
    sender.send_to(&[1; 100], link.address()?)?;
    let now = Instant::now();
    let mut buffer = [0; 128];
    link.pump(now, true)?;
    assert!(receiver.recv_from(&mut buffer).is_err());
    link.pump(now + Duration::from_millis(31), true)?;
    assert_eq!(receiver.recv_from(&mut buffer)?.0, 100);
    link.pump(now + Duration::from_millis(100), true)?;
    assert!(receiver.recv_from(&mut buffer).is_err());
    link.pump(now + Duration::from_millis(131), true)?;
    assert_eq!(receiver.recv_from(&mut buffer)?.0, 100);
    assert_eq!(link.up.duplicated, 1);
    assert_eq!(link.up.offered_bytes, 100);
    assert_eq!(link.up.delivered_bytes, 200);
    link.conditions(
        Conditions {
            loss: 10000,
            ..Default::default()
        },
        Conditions::default(),
    )?;
    sender.send_to(&[2; 100], link.address()?)?;
    link.pump(now + Duration::from_millis(200), true)?;
    assert_eq!(link.up.dropped, 1);
    Ok(())
}

#[test]
fn local_listen_dedicated_late_join_adverse_and_bandwidth() -> Result<()> {
    for mode in [SessionMode::Dedicated, SessionMode::Listen] {
        run(mode)?;
    }
    Ok(())
}
fn run(mode: SessionMode) -> Result<()> {
    let mut links = (0..4)
        .map(|p| Link::new(21668 + p * 997, Conditions::ADVERSE))
        .collect::<Result<Vec<_>>>()?;
    let proxies = links
        .iter()
        .map(Link::address)
        .collect::<Result<Vec<_>>>()?;
    let remote_start = usize::from(mode == SessionMode::Listen);
    let mut server = Server::<Movement>::new(
        session()?,
        policies()?,
        ServerConfig {
            mode,
            max_clients: 4,
            start_clients: 3 - remote_start,
            expected_addresses: proxies.clone(),
            tick_limit: Some(600),
            ..Default::default()
        },
    )?;
    configure(&mut server)?;
    server.position("e_0003", [100.0, 100.0])?;
    server.view(ConnectionView {
        player: PlayerId(remote_start as u64),
        position: [0.0; 2],
        radius: 10.0,
        team: None,
        team_only: false,
        bytes_per_second: 3200,
    })?;
    if mode == SessionMode::Listen {
        server.intent_host(Intent {
            tick: 60,
            sequence: 1,
            command: "heal".into(),
            target: "player0".into(),
            payload: serde_json::to_vec(&json!({"count":1}))?,
        })?;
    }
    server.record(build(), links.iter().map(Link::seed).collect())?;
    let address = server.address()?;
    let mut tokens = BTreeMap::new();
    for (p, link) in links.iter_mut().enumerate().skip(remote_start) {
        link.set_server(address);
        tokens.insert(p, server.token(PlayerId(p as u64), proxies[p])?);
    }
    let mut clients = BTreeMap::new();
    for (p, &endpoint) in proxies.iter().enumerate().take(3).skip(remote_start) {
        let mut client = Client::<Movement>::new(
            policies()?,
            ClientConfig {
                player: PlayerId(p as u64),
                endpoint,
                hz: 60,
                tick_limit: Some(600),
            },
            tokens.remove(&p).unwrap(),
        )?;
        client.input_source(script);
        if p == remote_start {
            client.perturb_prediction(330, |s| s.x += 250);
        }
        clients.insert(p, client);
    }
    if mode == SessionMode::Listen {
        for tick in 1..=600 {
            server.inject_host(
                tick,
                Injection {
                    player: PlayerId(0),
                    action: "Move".into(),
                    value: script(tick, PlayerId(0)).unwrap(),
                    hold_ticks: 1,
                },
            )?;
        }
    }
    let start = Instant::now();
    let mut heal = std::collections::BTreeSet::new();
    let mut late_baseline = 0;
    let mut measure_start = None;
    let mut measure_end = None;
    let mut deferred_seen = false;
    loop {
        let now = Instant::now();
        if start.elapsed() > Duration::from_secs(30) {
            panic!(
                "convergence deadline {:?} at {}: {:?}",
                mode,
                server.session().tick(),
                clients
                    .iter_mut()
                    .map(|(p, c)| (p, c.status()))
                    .collect::<Vec<_>>()
            );
        }
        if server.session().tick() >= 180 && !clients.contains_key(&3) {
            let mut client = Client::<Movement>::new(
                policies()?,
                ClientConfig {
                    player: PlayerId(3),
                    endpoint: proxies[3],
                    hz: 60,
                    tick_limit: Some(600),
                },
                tokens.remove(&3).unwrap(),
            )?;
            client.input_source(script);
            clients.insert(3, client);
        }
        let measure = (300..540).contains(&server.session().tick());
        if measure && measure_start.is_none() {
            measure_start = Some(now);
        }
        if !measure && measure_start.is_some() && measure_end.is_none() {
            measure_end = Some(now);
        }
        for link in links.iter_mut().skip(remote_start) {
            link.pump(now, measure)?;
        }
        server.poll()?;
        if server.session().tick() >= 100 {
            server.position("e_0003", [0.0; 2])?;
        }
        for (&p, client) in &mut clients {
            client.poll()?;
            let status = client.status();
            deferred_seen |= status.pending_references > 0;
            if p == 3 && status.tick > 0 && late_baseline == 0 {
                late_baseline = status.tick;
                let door = client
                    .spawns()
                    .into_iter()
                    .find(|s| s.identity == "door")
                    .unwrap();
                assert_eq!(client.component(door.id, "Door")?.unwrap()["open"], true);
            }
            if status.tick >= 10 && heal.insert(p) {
                client.intent(Intent {
                    tick: if p == 3 { status.tick + 20 } else { 60 },
                    sequence: 1,
                    command: "heal".into(),
                    target: format!("player{p}"),
                    payload: serde_json::to_vec(&json!({"count":1}))?,
                })?;
            }
        }
        for link in links.iter_mut().skip(remote_start) {
            link.pump(now, measure)?;
        }
        if server.session().tick() == 600 {
            let spawns = clients.values_mut().next().unwrap().spawns();
            let converged = clients.iter_mut().all(|(&p, c)| {
                c.status().tick == 600
                    && spawns
                        .iter()
                        .filter(|s| s.identity.starts_with("player"))
                        .all(|s| {
                            let native = server
                                .session()
                                .world()
                                .get::<Motor>(server.session().entity(&s.identity).unwrap())
                                .unwrap();
                            c.state(s.id).as_ref() == Some(native)
                                && (s.owner != Some(PlayerId(p as u64))
                                    || c.view(s.id).as_ref() == Some(native))
                        })
            });
            if converged {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(late_baseline >= 180);
    assert_eq!(server.budgets()[&PlayerId(remote_start as u64)], 3200);
    assert!(
        deferred_seen,
        "interest-hidden reference must defer until its baseline enters"
    );
    assert_eq!(
        server.session().component("door", "Door")?.unwrap()["open"],
        true
    );
    for p in 0..4 {
        assert_eq!(
            server
                .session()
                .component(&format!("player{p}"), "Motor")?
                .unwrap(),
            json!({"x":0,"z":0})
        );
        assert_eq!(
            server
                .session()
                .component(&format!("player{p}"), "Inventory")?
                .unwrap()["hp"],
            60
        );
    }
    let mut rollback = 0;
    let mut corrections = 0;
    let mut max_correction = 0.0_f64;
    for (&p, client) in &mut clients {
        assert_eq!(client.spawns().len(), 50);
        let status = client.status();
        assert_eq!(status.privacy_leaks, 0);
        assert!(status.private_samples > 0);
        assert_eq!(status.predicted, 1);
        assert_eq!(status.interpolated, 3);
        rollback += status.rollbacks;
        corrections += status.corrections.len();
        max_correction = max_correction.max(status.corrections.into_iter().fold(0.0, f64::max));
        for spawn in client.spawns() {
            assert!(!spawn.projected, "these lifetimes were created at runtime");
            if spawn.identity.starts_with("player") {
                let inv = client.component(spawn.id, "Inventory")?.unwrap();
                assert_eq!(inv["secret"], serde_json::Value::Null);
                if spawn.owner == Some(PlayerId(p as u64)) {
                    assert_eq!(inv["items"], json!((1_u64 << p) - 1));
                } else {
                    assert!(inv.get("items").is_none());
                }
            }
        }
    }
    assert!(rollback > 0);
    assert!(corrections > 0);
    let mut local = session()?;
    let binary = server.trace().unwrap().to_bytes()?;
    let trace = Trace::from_bytes(&binary)?;
    assert_eq!(
        trace.link_seeds,
        links.iter().map(Link::seed).collect::<Vec<_>>()
    );
    assert_eq!(trace.replay(&mut local)?, None);
    assert_eq!(local.state_hash()?, server.session().state_hash()?);
    let duration = measure_end
        .unwrap()
        .duration_since(measure_start.unwrap())
        .as_secs_f64();
    let down = links
        .iter()
        .skip(remote_start)
        .map(|l| l.down.measured_bytes as f64 / duration)
        .sum::<f64>()
        / (4 - remote_start) as f64;
    let up = links
        .iter()
        .skip(remote_start)
        .map(|l| l.up.measured_bytes as f64 / duration)
        .sum::<f64>()
        / (4 - remote_start) as f64;
    println!(
        "{mode:?}: 4 players/50 entities down={down:.1} up={up:.1} B/s; rollbacks={rollback} corrections={corrections} max={max_correction} mm; late baseline={late_baseline}"
    );
    assert!(down <= 2980.0, "downstream bandwidth {down}");
    assert!(up <= 4830.0, "upstream bandwidth {up}");
    Ok(())
}
