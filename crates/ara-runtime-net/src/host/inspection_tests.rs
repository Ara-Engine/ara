use super::*;

fn host(clients: Vec<Option<Conditions>>) -> NetHost {
    let scene = ara_doc::AssetId::parse("sc_linkproof").unwrap();
    let mut workspace = ara_doc::Workspace::default();
    workspace.insert(
        "levels/link-proof.scene",
        ara_doc::Document::new("scene", scene.clone()),
    );
    let runtime = Runtime::new(
        ara_runtime::schema(&[]).unwrap(),
        workspace,
        scene,
        Box::new(ara_runtime::assets::MemoryAssets::default()),
        ara_runtime::RuntimeConfig::default(),
        vec![Box::new(NetPlugin)],
    )
    .unwrap();
    NetHost::new(
        runtime,
        HostConfig {
            dedicated: true,
            clients,
        },
    )
    .unwrap()
}

#[test]
fn snapshots_read_each_live_proxy_without_advancing_the_authority() {
    let stress = Conditions {
        latency_ms: 200,
        jitter_ms: 50,
        loss: 200,
        ..Conditions::default()
    };
    let mut host = host(vec![None, None, Some(stress)]);
    let before = host.stats();
    assert_eq!(before.links.len(), 3);
    for (index, link) in before.links.iter().enumerate() {
        assert_eq!(link.client_index, index);
        assert_eq!(link.player, index as u64 + 2);
        let expected = if index == 2 {
            stress
        } else {
            Conditions::default()
        };
        assert_eq!([link.up, link.down], [expected; 2]);
    }
    let down = Conditions {
        latency_ms: 17,
        bandwidth: 32000,
        ..Conditions::default()
    };
    host.links[1]
        .conditions(Conditions::default(), down)
        .unwrap();
    let actual = host.stats();
    assert_eq!(
        actual.links[1].down, down,
        "Read the live proxy, not launch config"
    );
    assert_eq!(actual.links[0], before.links[0]);
    assert_eq!(actual.links[2], before.links[2]);
    assert_eq!(actual.tick, before.tick);
    assert_eq!(actual.connections, before.connections);
    assert_eq!(
        host.stats(),
        actual,
        "Repeated inspection cannot mutate counters"
    );
    let json = serde_json::to_value(actual).unwrap();
    assert_eq!(json["links"][1]["clientIndex"], 1);
    assert_eq!(json["links"][1]["player"], 3);
    assert_eq!(json["links"][1]["down"]["bandwidth"], 32000);
    assert!(json["links"][1]["upStats"].is_object());
    assert!(json["links"][1]["downStats"].is_object());
}

#[test]
fn live_datagram_counters_belong_to_the_same_proxy_and_empty_shape_is_preserved() {
    let empty = host(Vec::new());
    assert!(
        serde_json::to_value(empty.stats())
            .unwrap()
            .get("links")
            .is_none()
    );
    let mut host = host(vec![None, None]);
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let payload = b"link-inspection-test";
    socket
        .send_to(payload, host.links[0].address().unwrap())
        .unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    while host.links[0].up.packets == 0 && Instant::now() < deadline {
        host.links[0].pump(Instant::now(), true).unwrap();
        std::thread::yield_now();
    }
    let actual = host.stats();
    assert_eq!(actual.links[0].up_stats.packets, 1);
    assert_eq!(actual.links[0].up_stats.offered_bytes, payload.len() as u64);
    assert_eq!(actual.links[1].up_stats.packets, 0);
    assert_eq!(actual.up_bytes, actual.links[0].up_stats.delivered_bytes);
    assert_eq!(actual.tick, 0, "Inspection did not step gameplay");
}
