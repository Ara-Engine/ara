#![allow(clippy::unwrap_used)]
mod support;
use ara_input::PlayerId;
use ara_net::*;
use ara_sim::SessionMode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use support::*;

#[derive(Serialize, Deserialize)]
struct Start {
    directory: PathBuf,
    player: u64,
    proxies: Vec<std::net::SocketAddr>,
}
#[derive(Serialize, Deserialize)]
struct Expected {
    motors: BTreeMap<String, Motor>,
    inventory: BTreeMap<String, serde_json::Value>,
}
fn write(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    let path = path.as_ref();
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(value)?)),
    )?;
    fs::rename(temporary, path)?;
    Ok(())
}
fn read<T: serde::de::DeserializeOwned>(path: impl AsRef<Path>) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

#[test]
#[ignore = "private split-token peer for the warm handshake regression"]
fn split_token_worker() -> Result<()> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(std::env::var("ARA_NET_POOL_ADDR").unwrap())?;
    stream.set_nodelay(true)?;
    let token = std::env::var("ARA_NET_POOL_TOKEN").unwrap();
    let (first, second) = token.as_bytes().split_at(token.len() / 2);
    stream.write_all(first)?;
    std::thread::sleep(Duration::from_millis(100));
    stream.write_all(second)?;
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let mut payload = vec![0; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut payload)?;
    let (role, payload): (PlayerRole, Vec<u8>) = postcard::from_bytes(&payload)?;
    assert_eq!(role, PlayerRole::Dedicated);
    assert_eq!(payload, b"split-token");
    Ok(())
}

#[test]
fn warm_pool_accepts_a_delayed_split_readiness_token() -> Result<()> {
    let mut pool = PlayerPool::warm(
        &std::env::current_exe()?,
        &[
            "--exact".into(),
            "split_token_worker".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        1,
    )?;
    let mut players = pool.launch(
        &LaunchPlan {
            mode: SessionMode::Dedicated,
            clients: 0,
        },
        vec![b"split-token".to_vec()],
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !players.finished()? {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
#[ignore = "private single-use worker entry point, launched by the agent-style harness"]
fn player_worker_entry() -> Result<()> {
    player_worker(|role, payload| {
        let start: Start = postcard::from_bytes(&payload)?;
        let result = match role {
            PlayerRole::Dedicated => authority(&start),
            PlayerRole::Client => bot(&start),
            PlayerRole::Host => Err(NetError("dedicated gate only".into())),
        };
        if let Err(e) = &result {
            let _ = fs::write(
                start
                    .directory
                    .join(format!("error-{:?}-{}.txt", role, start.player)),
                e.to_string(),
            );
        }
        result
    })
}
fn authority(start: &Start) -> Result<()> {
    let mut server = Server::<Movement>::new(
        session()?,
        policies()?,
        ServerConfig {
            max_clients: 3,
            start_clients: 3,
            expected_addresses: start.proxies.clone(),
            tick_limit: Some(600),
            ..Default::default()
        },
    )?;
    configure(&mut server)?;
    server.record(build(), (0..3).map(|p| 21668 + p * 997).collect())?;
    for p in 0..3 {
        fs::write(
            start.directory.join(format!("token-{p}.bin")),
            server
                .token(PlayerId(p), start.proxies[p as usize])?
                .into_bytes(),
        )?;
    }
    write(start.directory.join("ready.json"), &server.address()?)?;
    let boot = Instant::now();
    let mut final_written = false;
    loop {
        if boot.elapsed() > Duration::from_secs(30) {
            return Err(NetError(format!(
                "server deadline at tick {}",
                server.session().tick()
            )));
        }
        server.poll()?;
        if server.session().tick() > 0 && !start.directory.join("started").exists() {
            fs::write(start.directory.join("started"), b"1")?;
        }
        if server.session().tick() == 600 && !final_written {
            let mut expected = Expected {
                motors: BTreeMap::new(),
                inventory: BTreeMap::new(),
            };
            for p in 0..4 {
                let identity = format!("player{p}");
                let entity = server.session().entity(&identity).unwrap();
                expected.motors.insert(
                    identity.clone(),
                    *server.session().world().get::<Motor>(entity).unwrap(),
                );
                expected.inventory.insert(
                    identity.clone(),
                    server.session().component(&identity, "Inventory")?.unwrap(),
                );
            }
            write(start.directory.join("expected.json"), &expected)?;
            let mut replay = session()?;
            assert_eq!(server.trace().unwrap().replay(&mut replay)?, None);
            assert_eq!(replay.state_hash()?, server.session().state_hash()?);
            write(
                start.directory.join("server.json"),
                &json!({"tick":600,"hash":server.session().state_hash()?.to_string(),"replay":true,"fields":server.profile()}),
            )?;
            final_written = true;
        }
        if final_written && (0..3).all(|p| start.directory.join(format!("ack-{p}")).exists()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
fn bot(start: &Start) -> Result<()> {
    let boot = Instant::now();
    while !start.directory.join("ready.json").exists() {
        if boot.elapsed() > Duration::from_secs(10) {
            return Err(NetError("server readiness timeout".into()));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let player = PlayerId(start.player);
    let endpoint = start.proxies[start.player as usize];
    let token = JoinToken::from_bytes(fs::read(
        start.directory.join(format!("token-{}.bin", start.player)),
    )?)?;
    let mut client = Client::<Movement>::new(
        policies()?,
        ClientConfig {
            player,
            endpoint,
            hz: 60,
            tick_limit: Some(600),
        },
        token,
    )?;
    client.input_source(script);
    if start.player == 0 {
        // Arm before the first predicted baseline; arrival can start beyond tick one.
        client.perturb_prediction(1, |motor| motor.x += 250);
    }
    let mut sent = false;
    loop {
        client.poll()?;
        let status = client.status();
        if status.tick >= 10 && !sent {
            client.intent(Intent {
                tick: 60,
                sequence: 1,
                command: "heal".into(),
                target: format!("player{}", start.player),
                payload: serde_json::to_vec(&json!({"count":1}))?,
            })?;
            sent = true;
        }
        if status.tick == 600 && start.directory.join("expected.json").exists() {
            let expected: Expected = read(start.directory.join("expected.json"))?;
            let mut converged = true;
            for spawn in client.spawns() {
                if let Some(motor) = expected.motors.get(&spawn.identity) {
                    converged &= client.state(spawn.id).as_ref() == Some(motor);
                    if spawn.owner == Some(player) {
                        converged &= client.view(spawn.id).as_ref() == Some(motor);
                    }
                    let inventory = client.component(spawn.id, "Inventory")?.unwrap();
                    assert!(inventory.get("secret").is_none());
                    if spawn.owner == Some(player) {
                        converged &=
                            inventory["items"] == expected.inventory[&spawn.identity]["items"];
                    } else {
                        assert!(inventory.get("items").is_none());
                    }
                }
            }
            if converged && client.spawns().len() == 50 {
                assert_eq!(status.privacy_leaks, 0);
                assert!(status.private_samples > 0);
                assert_eq!(status.predicted, 1);
                assert_eq!(status.interpolated, 3);
                if start.player == 0 {
                    assert!(status.rollbacks > 0);
                }
                write(
                    start
                        .directory
                        .join(format!("client-{}.json", start.player)),
                    &status,
                )?;
                fs::write(start.directory.join(format!("ack-{}", start.player)), b"1")?;
                // Keep polling briefly so the final reliable input/ACK traffic drains.
                let until = Instant::now() + Duration::from_millis(200);
                while Instant::now() < until {
                    client.poll()?;
                    std::thread::sleep(Duration::from_millis(2));
                }
                return Ok(());
            }
        }
        if boot.elapsed() > Duration::from_secs(30) {
            return Err(NetError(format!(
                "client {} convergence timeout: {status:?}",
                start.player
            )));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn agent_harness_prespawned_dedicated_plus_three_bots() -> Result<()> {
    let directory = std::env::temp_dir().join(format!(
        "ara-net-process-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory)?;
    let mut links = (0..3)
        .map(|p| Link::new(21668 + p * 997, Conditions::ADVERSE))
        .collect::<Result<Vec<_>>>()?;
    let proxies = links
        .iter()
        .map(Link::address)
        .collect::<Result<Vec<_>>>()?;
    let mut pool = PlayerPool::warm(
        &std::env::current_exe()?,
        &[
            "--exact".into(),
            "player_worker_entry".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        4,
    )?;
    let payloads = (0_u64..4)
        .map(|n| {
            postcard::to_allocvec(&Start {
                directory: directory.clone(),
                player: n.saturating_sub(1),
                proxies: proxies.clone(),
            })
            .map_err(NetError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut players = pool.launch(
        &LaunchPlan {
            mode: SessionMode::Dedicated,
            clients: 3,
        },
        payloads,
    )?;
    assert_eq!(pool.available(), 0);
    assert_eq!(players.ids().len(), 4);
    let boot = Instant::now();
    let mut configured = false;
    loop {
        if !configured && directory.join("ready.json").exists() {
            let address = read(directory.join("ready.json"))?;
            for link in &mut links {
                link.set_server(address);
            }
            configured = true;
        }
        let now = Instant::now();
        for link in &mut links {
            link.pump(now, false)?;
        }
        match players.finished() {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => {
                let details = fs::read_dir(&directory)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with("error-"))
                    .filter_map(|e| fs::read_to_string(e.path()).ok())
                    .collect::<Vec<_>>();
                panic!("{e}: {details:?}; {}", directory.display());
            }
        }
        if boot.elapsed() > Duration::from_secs(40) {
            panic!("agent harness timeout: {}", directory.display());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let server: serde_json::Value = read(directory.join("server.json"))?;
    assert_eq!(server["replay"], true);
    for p in 0..3 {
        let status: ClientStatus = read(directory.join(format!("client-{p}.json")))?;
        assert_eq!(status.tick, 600);
        assert_eq!(status.privacy_leaks, 0);
    }
    players.stop();
    println!(
        "Agent process gate: dedicated + 3 bots, encrypted adverse UDP, convergence/replay/privacy PASS; artifacts {}",
        directory.display()
    );
    Ok(())
}
