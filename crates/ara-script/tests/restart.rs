//! L4: a change the process cannot apply. The engine bindings (`Ara.Runtime`, in the
//! host's default context) changing on disk is reported as an L4 restart request for
//! every module, and the game keeps running its last good code. This test runs the host
//! from a private copy of `Ara.Host`'s output so it can replace `Ara.Runtime.dll`.

mod support;

use ara_script::abi::RELOAD_L4;
use ara_script::codes;
use serde_json::json;
use support::Live;

const DIAL: &str = r#"namespace Dials;

/// <summary>Ticks seen.</summary>
[Component]
public partial struct Dial
{
    /// <summary>Ticks.</summary>
    public int Ticks;
}

public static partial class Turning
{
    [Query]
    static void Run(ref Dial d) => d.Ticks += 1;
}
"#;

#[test]
fn engine_binding_changes_request_a_restart_and_keep_the_game_running() {
    let _clr = support::clr_lock();
    let root = Live::write("restart", &[("Dials", DIAL, &[])]);
    let host_dir = root.join("host");
    std::fs::create_dir_all(&host_dir).expect("host dir");
    for f in std::fs::read_dir(support::bin("Ara.Host")).expect("Ara.Host output") {
        let f = f.expect("entry").path();
        if f.is_file() {
            std::fs::copy(&f, host_dir.join(f.file_name().expect("name"))).expect("copy");
        }
    }
    let mut world = bevy_ecs::world::World::new();
    let (db, engine) = support::engine();
    let mut config = ara_script::ScriptConfig::jit(&host_dir);
    config.compile_server = Some(support::bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
    let host = ara_script::ScriptHost::start(config, db, &engine, &mut world).expect("host starts");
    let mut s = Live {
        host,
        world,
        root,
        frame: 0,
        reports: Vec::new(),
        diagnostics: Vec::new(),
    };
    let spec = ara_script::ModuleSpec::from_dir(&s.root.join("Dials")).expect("module");
    s.host
        .load_modules(&mut s.world, &[spec])
        .unwrap_or_else(|e| panic!("{e:#?}"));
    let e = s.spawn(&[("Dial", json!({}))]);
    s.until("the initial code", |s| s.int(e, "Dial", "ticks") > 0);

    let peer_root = Live::write("restart-peer", &[("Dials", DIAL, &[])]);
    let mut peer_world = bevy_ecs::world::World::new();
    let (peer_db, peer_engine) = support::engine();
    let mut peer_config = ara_script::ScriptConfig::jit(&host_dir);
    peer_config.compile_server =
        Some(support::bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
    let peer_host =
        ara_script::ScriptHost::start(peer_config, peer_db, &peer_engine, &mut peer_world)
            .expect("peer starts in the shared CLR");
    let mut peer = Live {
        host: peer_host,
        world: peer_world,
        root: peer_root,
        frame: 0,
        reports: Vec::new(),
        diagnostics: Vec::new(),
    };
    let peer_spec =
        ara_script::ModuleSpec::from_dir(&peer.root.join("Dials")).expect("peer module");
    peer.host
        .load_modules(&mut peer.world, &[peer_spec])
        .expect("peer modules");
    let peer_entity = peer.spawn(&[("Dial", json!({}))]);
    peer.run();

    // The loaded file cannot be overwritten on Windows, but it can be moved aside.
    let runtime = host_dir.join("Ara.Runtime.dll");
    std::fs::rename(&runtime, host_dir.join("Ara.Runtime.old")).expect("move the runtime aside");
    std::fs::copy(support::bin("Ara.Host").join("Ara.Host.dll"), &runtime)
        .expect("a different assembly");
    s.until("the L4 report", |s| s.count(RELOAD_L4) > 0);
    peer.until("the peer's independent L4 report", |s| {
        s.count(RELOAD_L4) > 0
    });
    assert!(
        s.diagnostics
            .iter()
            .any(|d| d.code == codes::RESTART_REQUIRED),
        "{:#?}",
        s.diagnostics
    );
    let ticks = s.int(e, "Dial", "ticks");
    s.tick();
    assert_eq!(
        s.int(e, "Dial", "ticks"),
        ticks + 1,
        "the last good code keeps running"
    );
    let root = s.root.clone();
    assert_eq!(s.host.shutdown(), 0);
    drop(s.world);
    let before = peer.int(peer_entity, "Dial", "ticks");
    peer.run();
    assert_eq!(peer.int(peer_entity, "Dial", "ticks"), before + 1);
    assert!(
        peer.diagnostics
            .iter()
            .any(|d| d.code == codes::RESTART_REQUIRED)
    );
    assert_eq!(peer.finish(), 0);
    let _ = std::fs::remove_dir_all(root);
}
