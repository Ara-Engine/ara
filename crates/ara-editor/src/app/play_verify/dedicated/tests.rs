//! CPU checks for native-proof predicates; the actual story still requires SDL and GPU output.
use super::*;

fn conditions(shaped: bool) -> Value {
    json!({"latencyMs":if shaped {200}else{0},"jitterMs":if shaped {50}else{0},"loss":if shaped {200}else{0},"duplication":0,"reordering":0,"bandwidth":0})
}
fn world() -> Value {
    let identities = [11, 22, 33];
    let links:Vec<_>=identities.into_iter().enumerate().map(|(index,player)|json!({
        "clientIndex":index,"player":player,"up":conditions(index==2),"down":conditions(index==2),
        "upStats":{"packets":10,"offered_bytes":1024,"delivered_bytes":900},
        "downStats":{"packets":12,"offered_bytes":2048,"delivered_bytes":1900}
    })).collect();
    let mut players = vec![
        json!({"role":"dedicated","pid":100,"surface":null,"status":{"net":{"connections":3,"links":links}}}),
    ];
    players.extend(identities.into_iter().enumerate().map(|(index,player)|json!({
        "role":"client","pid":101+index,"surface":{"generation":1,"width":640,"height":360},"status":{"net":{"player":player}}
    })));
    json!({"world":"play:native-dedicated","processMode":"separate","players":players})
}

#[test]
fn dedicated_topology_distinguishes_authority_surface_index_and_runtime_owner() {
    let world = world();
    let roles =
        proof::topology(&world).expect("One dedicated authority and three live client identities");
    assert_eq!(roles["authority"]["index"], 0);
    assert_eq!(roles["clients"][1]["index"], 2);
    assert_eq!(roles["clients"][1]["runtimePlayer"], 22);
    assert_eq!(
        proof::focused_client(&BTreeMap::from([("dedicated-topology".into(), roles)])),
        Some(2)
    );
    for damaged in [
        "authority-surface",
        "duplicate-pid",
        "missing-client-image",
        "wrong-role",
    ] {
        let mut changed = world.clone();
        match damaged {
            "authority-surface" => changed["players"][0]["surface"] = json!({"generation":1}),
            "duplicate-pid" => changed["players"][2]["pid"] = json!(100),
            "missing-client-image" => changed["players"][3]["surface"] = Value::Null,
            _ => changed["players"][0]["role"] = json!("host"),
        }
        assert!(proof::topology(&changed).is_none(), "{damaged}");
    }
}

#[test]
fn applied_link_proof_requires_actual_per_client_values_in_both_directions() {
    let original = world();
    let roles = proof::topology(&original).expect("Valid topology");
    let good = proof::links(&original, &roles);
    for field in [
        "inspectionAvailable",
        "clientIdentitiesMatch",
        "onlyClient3Shaped",
        "allThreeProxiesCarryTraffic",
    ] {
        assert_eq!(good[field], true, "{field}");
    }
    let mut reordered = original.clone();
    reordered["players"][0]["status"]["net"]["links"]
        .as_array_mut()
        .expect("Link array")
        .swap(0, 2);
    assert_eq!(
        proof::links(&reordered, &roles)["onlyClient3Shaped"],
        true,
        "Link identity comes from clientIndex, not array position"
    );
    let mut wrong = original.clone();
    wrong["players"][0]["status"]["net"]["links"][0]["up"] = conditions(true);
    assert_eq!(
        proof::links(&wrong, &roles)["onlyClient3Shaped"],
        false,
        "Shaping another client is a failure"
    );
    let mut one_direction = original.clone();
    one_direction["players"][0]["status"]["net"]["links"][2]["down"] = conditions(false);
    assert_eq!(
        proof::links(&one_direction, &roles)["onlyClient3Shaped"],
        false,
        "Both actual directions must match"
    );
    let mut missing = original;
    missing["players"][0]["status"]["net"]
        .as_object_mut()
        .expect("Authority net stats")
        .remove("links");
    missing["networkProfiles"] = json!({"1":"clean","2":"clean","3":"stress"});
    missing["requestedConditions"] =
        json!([conditions(false), conditions(false), conditions(true)]);
    let report = proof::links(&missing, &roles);
    assert_eq!(report["inspectionAvailable"], false);
    assert_eq!(
        report["onlyClient3Shaped"], false,
        "Requested preferences never replace live proxy inspection"
    );
}

fn observation() -> Value {
    let mut values = serde_json::Map::new();
    for (index, entity) in ENTITIES.iter().enumerate() {
        values.insert(
            format!("{entity}.Transform.position"),
            json!([index as f64 * 1.5, 0., 0.]),
        );
        values.insert(
            format!("{entity}.PlayerControl.player"),
            json!(11 * (index + 1)),
        );
    }
    json!({"tick":50,"values":values})
}

#[test]
fn client_input_isolation_uses_authority_reported_ownership_and_rejects_leaks() {
    let before = observation();
    let mut after = before.clone();
    after["values"]["e_player02.Transform.position"][2] = json!(2.);
    let result = proof::isolated_movement(&before, &after, 22);
    assert_eq!(result["owner"], "e_player02");
    assert_eq!(result["ownerMoved"], true);
    assert_eq!(result["otherActorsUnchanged"], true);
    after["values"][format!("{ACTOR}.Transform.position")][0] = json!(1.);
    assert_eq!(
        proof::isolated_movement(&before, &after, 22)["otherActorsUnchanged"],
        false
    );
    assert_eq!(
        proof::isolated_movement(&before, &after, 99)["ownerMoved"],
        false
    );
    after["values"]
        .as_object_mut()
        .expect("Read values")
        .remove("e_player04.Transform.position");
    assert_eq!(
        proof::isolated_movement(&before, &after, 22)["allPresent"],
        false
    );
}

#[test]
fn completed_tile_pixel_evidence_rejects_black_uniform_and_invalid_regions() {
    use image::{Rgba, RgbaImage};
    let rect = ara_ui::paint::Box2 {
        x: 0.,
        y: 0.,
        w: 80.,
        h: 48.,
    };
    let mut image = RgbaImage::from_pixel(80, 48, Rgba([0, 0, 0, 255]));
    assert_eq!(
        proof::pixel_samples(&image, rect, [80., 48.])["nonUniform"],
        false
    );
    image = RgbaImage::from_pixel(80, 48, Rgba([40, 90, 150, 255]));
    assert_eq!(
        proof::pixel_samples(&image, rect, [80., 48.])["nonUniform"],
        false
    );
    for y in 0..48 {
        for x in 40..80 {
            image.put_pixel(x, y, Rgba([100, 130, 180, 255]));
        }
    }
    let sampled = proof::pixel_samples(&image, rect, [80., 48.]);
    assert_eq!(sampled["samples"], 960);
    assert_eq!(sampled["nonUniform"], true);
    assert!(proof::pixel_samples(&image, rect, [f32::NAN, 48.]).is_null());
    assert!(
        proof::pixel_samples(&image, ara_ui::paint::Box2 { x: -100., ..rect }, [80., 48.])
            .is_null()
    );
}
