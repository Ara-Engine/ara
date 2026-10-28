//! Real group transport proves process identity and independently owned sessions.
#![allow(clippy::unwrap_used)]
use ara_play::{PlayScene, Role, SnapshotRef, Start, method};
use ara_play_host::pool::{Player, Pool, PoolConfig};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn output() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("ara-player/shared").join(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
fn scene(root: &std::path::Path) -> PlayScene {
    serde_json::from_value(json!({
        "format":1,"scene":"sc_group","seed":7319,"tick_hz":60,
        "assets":{},"cache":root.join("cache"),"nav_cache":null,"types":[],
        "documents":[{"path":"group.scene","kind":"scene","id":"sc_group","records":{
            "e_actor":{"name":"Actor","order":"a0","Transform":{},"PlayerControl":{"player":1,"camera":"e_camera"}},
            "e_camera":{"name":"Camera","order":"a1","Transform":{},"Camera":{},"FollowCamera":{"target":"e_actor","player":1}},
            "e_prop":{"name":"Prop","order":"a2","Transform":{"position":[0.,1.,0.]}}
        }}]
    })).unwrap()
}

#[test]
fn shared_network_worlds_patch_only_the_authority_and_adopt_control_from_replication() {
    let root = output();
    std::fs::create_dir_all(&root).unwrap();
    let mut pool = Pool::new(
        PoolConfig {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_ara-player")),
            logs: root.join("logs"),
            gpu_cache: root.join("gpu"),
            headless: 0,
            windowed: 0,
            size: [64, 64],
        },
        "shared-network-test".into(),
    )
    .unwrap();
    let mut peers = pool.take_shared(2, Duration::from_secs(60)).unwrap();
    let mut authority = peers.pop().unwrap();
    let mut mirror = peers.pop().unwrap();
    assert_eq!(authority.child.id(), mirror.child.id());
    let bytes = scene(&root).encode().unwrap();
    call(
        &mut authority,
        method::START,
        serde_json::to_value(Start {
            snapshot: SnapshotRef::Inline {
                bytes: bytes.clone(),
            },
            role: Role::Host,
            net: Some(ara_play::NetStart {
                players: 2,
                clients: vec![None],
                join: None,
            }),
            start_paused: false,
            resume: None,
        })
        .unwrap(),
    );
    let join =
        serde_json::from_value(call(&mut authority, method::JOIN, json!({"index":0}))).unwrap();
    call(
        &mut mirror,
        method::START,
        serde_json::to_value(Start {
            snapshot: SnapshotRef::Inline { bytes },
            role: Role::Client,
            net: Some(ara_play::NetStart {
                players: 2,
                clients: Vec::new(),
                join: Some(join),
            }),
            start_paused: false,
            resume: None,
        })
        .unwrap(),
    );
    let rejected=mirror.call(method::PATCH,&json!({"sets":[{"entity":"e_prop","path":"Transform.position","value":[99.,99.,99.]}]}),Duration::from_secs(10)).unwrap();
    assert!(
        rejected.is_err(),
        "mirrors must never independently execute authoring patches"
    );
    let patch = call(
        &mut authority,
        method::PATCH,
        json!({"sets":[{"entity":"e_prop","path":"Transform.position","value":[9.,2.,3.]}]}),
    );
    assert!(patch["appliedAtTick"].as_u64().is_some());
    let began = std::time::Instant::now();
    loop {
        let actual = read(&mut mirror, "e_prop.Transform.position");
        if actual == json!([9., 2., 3.]) {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "declared replicated Transform did not arrive: {actual}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let eject = call(
        &mut authority,
        method::CONTROL,
        json!({"owner":1,"entity":null}),
    );
    assert_eq!(eject["previous"], json!(["e_actor"]));
    let possess = call(
        &mut authority,
        method::CONTROL,
        json!({"owner":2,"entity":"e_actor"}),
    );
    assert_eq!(possess["previous"], json!(["e_player02"]));
    let began = std::time::Instant::now();
    loop {
        let actor = read(&mut mirror, "e_actor.PlayerControl.player");
        let released = read(&mut mirror, "e_player02.PlayerControl.player");
        let camera = read(&mut mirror, "e_camera.FollowCamera.target");
        if actor == 2 && released == 0 && camera == "e_actor" {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "mirror did not adopt replicated ownership/camera: actor={actor},released={released},camera={camera}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(root.join("network.json"),ara_json::to_canonical_string(&json!({"pid":authority.child.id(),"patch":patch,"eject":eject,"possess":possess,"independentMirrorPatchRejected":true}))).unwrap();
}
fn call(player: &mut Player, method: &str, params: Value) -> Value {
    player
        .call(method, &params, Duration::from_secs(30))
        .unwrap()
        .unwrap()
}
fn read(player: &mut Player, path: &str) -> Value {
    call(player, method::READ, json!({"paths":[path]}))["values"][path].clone()
}

#[test]
fn shared_players_have_one_pid_and_independent_ticks_input_ecs_and_navigation() {
    let root = output();
    std::fs::create_dir_all(&root).unwrap();
    let mut pool = Pool::new(
        PoolConfig {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_ara-player")),
            logs: root.join("logs"),
            gpu_cache: root.join("gpu"),
            headless: 0,
            windowed: 0,
            size: [64, 64],
        },
        "shared-isolation-test".into(),
    )
    .unwrap();
    let mut players = pool.take_shared(3, Duration::from_secs(60)).unwrap();
    let pid = players[0].child.id();
    assert!(
        players
            .iter()
            .all(|p| p.child.id() == pid && p.child.is_shared() && !p.hello.window)
    );
    let snapshot = scene(&root).encode().unwrap();
    for player in &mut players {
        let started = call(
            player,
            method::START,
            serde_json::to_value(Start {
                snapshot: SnapshotRef::Inline {
                    bytes: snapshot.clone(),
                },
                role: Role::Local,
                net: None,
                start_paused: true,
                resume: None,
            })
            .unwrap(),
        );
        assert_eq!(started["tick"], 0);
    }
    let before = call(&mut players[1], method::CAPTURE, json!({}));
    let input = json!({"ticks":30,"inputs":[{"action":"Look","value":[12.,0.],"holdTicks":1}]});
    let first = call(&mut players[0], method::STEP, input.clone());
    let sibling = call(&mut players[1], method::CAPTURE, json!({}));
    assert_eq!(
        sibling["tick"], 0,
        "one world's stepping must not tick its sibling"
    );
    assert_eq!(
        sibling["hash"], before["hash"],
        "one world's actions must not alter sibling ECS/RNG"
    );
    assert_ne!(
        read(&mut players[0], "e_camera.FollowCamera.yaw"),
        read(&mut players[1], "e_camera.FollowCamera.yaw")
    );
    let second = call(&mut players[1], method::STEP, input);
    assert_eq!(
        first["hash"], second["hash"],
        "same seed, ticks and inputs must converge independently"
    );
    call(
        &mut players[0],
        method::PATCH,
        json!({"sets":[{"entity":"e_actor","path":"Transform.position","value":[9.,2.,3.]}]}),
    );
    assert_ne!(
        read(&mut players[0], "e_actor.Transform.position"),
        read(&mut players[1], "e_actor.Transform.position")
    );
    let view = call(
        &mut players[0],
        method::VIEW,
        json!({"camera":{"position":[4.,5.,6.],"target":[0.,0.,0.],"fovYDegrees":55.},"navigationSpeed":16.}),
    );
    assert_eq!(view["mode"], "spectator");
    assert_eq!(view["navigationSpeed"], 16.);
    let sibling = call(
        &mut players[1],
        method::VIEW,
        json!({"captureCurrent":true}),
    );
    assert_eq!(
        sibling["navigationSpeed"], 4.,
        "navigation documents belong to each player view"
    );
    assert_ne!(view["camera"], sibling["camera"]);
    let untouched = call(&mut players[2], method::CAPTURE, json!({}));
    assert_eq!(untouched["tick"], 0);
    assert_eq!(untouched["hash"], before["hash"]);
    std::fs::write(root.join("isolation.json"),ara_json::to_canonical_string(&json!({"pid":pid,"worlds":players.len(),"sameSeedHash":first["hash"],"untouched":untouched,"view":view,"siblingView":sibling}))).unwrap();
    // The separate mode keeps its process-crash isolation contract.
    let separate = pool.take(2, false, Duration::from_secs(60)).unwrap();
    assert_ne!(separate[0].child.id(), separate[1].child.id());
    assert!(
        separate
            .iter()
            .all(|p| !p.child.is_shared() && p.child.id() != pid)
    );
}
