//! Roles, live proxy state and authority observations are independent proof checks.
use super::*;
use ara_ui::{
    paint::{Box2, Draw},
    source::stable_id,
};

pub(super) fn topology(world: &Value) -> Option<Value> {
    if world["processMode"] != "separate" {
        return None;
    }
    let players = world["players"].as_array()?;
    if players.len() != 4
        || players[0]["role"] != "dedicated"
        || !players[0]["surface"].is_null()
        || players[0]["status"]["net"]["connections"].as_u64() != Some(3)
    {
        return None;
    }
    let authority = players[0]["pid"].as_u64().filter(|pid| *pid > 0)?;
    let mut pids = BTreeSet::from([authority]);
    let mut identities = BTreeSet::new();
    let mut clients = Vec::new();
    for (index, client) in players.iter().enumerate().skip(1) {
        if client["role"] != "client" {
            return None;
        }
        let pid = client["pid"].as_u64().filter(|pid| *pid > 0)?;
        let identity = client["status"]["net"]["player"]
            .as_u64()
            .filter(|player| *player > 0)?;
        if !pids.insert(pid)
            || !identities.insert(identity)
            || client["surface"]["generation"].as_u64().unwrap_or(0) == 0
            || client["surface"]["width"].as_u64().unwrap_or(0) == 0
            || client["surface"]["height"].as_u64().unwrap_or(0) == 0
        {
            return None;
        }
        clients.push(json!({"index":index,"runtimePlayer":identity,"pid":pid,"role":"client","surface":client["surface"]}));
    }
    Some(
        json!({"authority":{"index":0,"pid":authority,"role":"dedicated","surface":null},"clients":clients,"processMode":"separate"}),
    )
}

pub(super) fn focused_client(proofs: &BTreeMap<String, Value>) -> Option<usize> {
    usize::try_from(proofs.get("dedicated-topology")?["clients"][1]["index"].as_u64()?).ok()
}

fn expected_conditions(shaped: bool) -> Value {
    json!({"latencyMs":if shaped {200}else{0},"jitterMs":if shaped {50}else{0},"loss":if shaped {200}else{0},"duplication":0,"reordering":0,"bandwidth":0})
}

pub(super) fn links(world: &Value, topology: &Value) -> Value {
    let authority = topology["authority"]["index"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok());
    let actual =
        authority.and_then(|index| world["players"][index]["status"]["net"]["links"].as_array());
    let available = actual.is_some_and(|links| links.len() == 3);
    let mut identities = available;
    let mut conditions = available;
    let mut traffic = available;
    for client in 0..3 {
        let link = actual.and_then(|links| {
            links
                .iter()
                .find(|link| link["clientIndex"].as_u64() == Some(client as u64))
        });
        identities &=
            link.is_some_and(|link| link["player"] == topology["clients"][client]["runtimePlayer"]);
        conditions &= link.is_some_and(|link| {
            link["up"] == expected_conditions(client == 2)
                && link["down"] == expected_conditions(client == 2)
        });
        // DirectionStats intentionally retains its existing snake_case wire fields.
        traffic &= link.is_some_and(|link| {
            ["upStats", "downStats"].iter().all(|direction| {
                link[*direction]["packets"].as_u64().unwrap_or(0) > 0
                    && link[*direction]["delivered_bytes"].as_u64().unwrap_or(0) > 0
            })
        });
    }
    json!({"inspectionAvailable":available,"clientIdentitiesMatch":identities,"onlyClient3Shaped":conditions,"allThreeProxiesCarryTraffic":traffic,
        "source":"live authority NetHost::stats -> Link::applied_conditions and DirectionStats, via readonly play.status heartbeat",
        "actual":actual,"expectedClient3":expected_conditions(true)})
}

fn owner_entity(before: &Value, player: u64) -> Option<&'static str> {
    let mut matches = ENTITIES.iter().copied().filter(|entity| {
        before["values"][format!("{entity}.PlayerControl.player")].as_u64() == Some(player)
    });
    let entity = matches.next()?;
    matches.next().is_none().then_some(entity)
}

pub(super) fn isolated_movement(before: &Value, after: &Value, player: u64) -> Value {
    let owner = owner_entity(before, player);
    let all_present = ENTITIES
        .iter()
        .all(|entity| horizontal(before, entity).is_some() && horizontal(after, entity).is_some());
    let owner_moved = owner
        .is_some_and(|entity| moved(before, after, entity).is_some_and(|distance| distance > 0.1));
    let others_still = owner.is_some_and(|owner| {
        ENTITIES
            .iter()
            .filter(|entity| **entity != owner)
            .all(|entity| moved(before, after, entity).is_some_and(|distance| distance < 0.05))
    });
    json!({"owner":owner,"runtimePlayer":player,"allPresent":all_present,"ownerMoved":owner_moved,"otherActorsUnchanged":others_still,
        "displacements":ENTITIES.iter().map(|entity|json!({"entity":entity,"horizontalMetres":moved(before,after,entity)})).collect::<Vec<_>>()})
}

fn stationary(before: &Value, after: &Value) -> bool {
    ENTITIES
        .iter()
        .all(|entity| moved(before, after, entity).is_some_and(|distance| distance < 0.05))
}

pub(super) fn pixel_samples(pixels: &image::RgbaImage, rect: Box2, logical: [f32; 2]) -> Value {
    if pixels.width() == 0
        || pixels.height() == 0
        || ![rect.x, rect.y, rect.w, rect.h, logical[0], logical[1]]
            .into_iter()
            .all(f32::is_finite)
        || rect.w < 32.
        || rect.h < 18.
        || logical.iter().any(|size| *size <= 0.)
    {
        return Value::Null;
    }
    let scale = [
        pixels.width() as f32 / logical[0],
        pixels.height() as f32 / logical[1],
    ];
    let mut colors = BTreeSet::new();
    let mut hash = blake3::Hasher::new();
    let mut samples = 0;
    for y in 0..24 {
        for x in 0..40 {
            // Stay inside the live image, excluding frame edges and capture rings.
            let point = [
                rect.x + 3. + (x as f32 + 0.5) * (rect.w - 6.) / 40.,
                rect.y + 3. + (y as f32 + 0.5) * (rect.h - 6.) / 24.,
            ];
            let px = (point[0] * scale[0]).floor();
            let py = (point[1] * scale[1]).floor();
            if px < 0. || py < 0. || px >= pixels.width() as f32 || py >= pixels.height() as f32 {
                return Value::Null;
            }
            let color = pixels.get_pixel(px as u32, py as u32).0;
            colors.insert([color[0], color[1], color[2]]);
            hash.update(&color);
            samples += 1;
        }
    }
    json!({"algorithm":"BLAKE3 of 40x24 inset RGBA samples from completed native UI image","samples":samples,"colors":colors.len(),"nonUniform":colors.len()>1,"digest":hash.finalize().to_hex().to_string()})
}

pub(super) fn tiles(
    shell: &Shell,
    proofs: &BTreeMap<String, Value>,
    pixels: &image::RgbaImage,
) -> Vec<Value> {
    let Some(clients) = proofs
        .get("dedicated-topology")
        .and_then(|topology| topology["clients"].as_array())
    else {
        return Vec::new();
    };
    clients.iter().map(|client| {
        let Some(index)=client["index"].as_u64().and_then(|index|usize::try_from(index).ok()) else {return Value::Null;};
        let image_id=format!("game-image-{index}");
        let node=shell.shown_node(&image_id).map(|index|&shell.ui.nodes[index]);
        let rect=node.map(|node|node.rect.intersection(node.clip));
        let texture=game::player_texture(index);
        let drawn=shell.ui.display.commands.iter().any(|draw|matches!(draw,Draw::Image{node,image,..} if *node==stable_id(&image_id) && *image==texture));
        let live=shell.play_controls.players.get(&index).is_some_and(|info|info.live && !info.size.contains(&0));
        json!({"index":index,"runtimePlayer":client["runtimePlayer"],"pid":client["pid"],"texture":texture,"rect":rect,"live":live,"nativeImageDrawn":drawn,
            "pixels":rect.map(|rect|pixel_samples(pixels,rect,shell.ui.size))})
    }).collect()
}

fn rendered(trace: &Value) -> bool {
    let Some(tiles) = trace["tiles"].as_array() else {
        return false;
    };
    let identities: BTreeSet<_> = tiles
        .iter()
        .filter_map(|tile| tile["index"].as_u64())
        .collect();
    let textures: BTreeSet<_> = tiles
        .iter()
        .filter_map(|tile| tile["texture"].as_u64())
        .collect();
    tiles.len() == 3
        && identities.len() == 3
        && textures.len() == 3
        && tiles.iter().all(|tile| {
            tile["live"] == true
                && tile["nativeImageDrawn"] == true
                && tile["pixels"]["nonUniform"] == true
        })
}

pub(super) fn checks(editor: &Editor, check: &PlayCheck) -> Value {
    let proof = |label: &str| check.proofs.get(label).unwrap_or(&Value::Null);
    let trace = |stage: &str| {
        check
            .trace
            .iter()
            .find(|row| row["stage"] == stage)
            .unwrap_or(&Value::Null)
    };
    let roles = proof("dedicated-topology");
    let owner = roles["clients"][1]["runtimePlayer"].as_u64().unwrap_or(0);
    let focus = roles["clients"][1]["index"].as_u64();
    let walking = isolated_movement(proof("dedicated-before"), proof("dedicated-client2"), owner);
    let sideways = isolated_movement(
        proof("dedicated-key-released"),
        proof("dedicated-focuslost"),
        owner,
    );
    let applied = links(proof("applied-link-status"), roles);
    let source_scene = configuration::scene(editor).ok();
    let stopped = proof("stop-status");
    let world = &proof("processes")["world"];
    let all_revisions = check
        .trace
        .iter()
        .filter(|row| row["stage"] != "before" && row["stage"] != "configured")
        .all(|row| row["revision"].as_u64() == Some(check.before));
    json!({
        "dedicatedAuthorityAndThreeDistinctClientProcesses":topology(proof("processes")).is_some(),
        "threeIndependentRenderingClientViewports":rendered(trace("ready")) && rendered(trace("client2-captured")),
        "nativeCaptureTargetsSecondActualClient":focus.is_some() && trace("client2-captured")["runtime"]["controlledPlayer"].as_u64()==focus && trace("client2-captured")["runtime"]["captured"]==true,
        "onlyClient2ActorMovedFromNativeW":walking["allPresent"]==true && walking["ownerMoved"]==true && walking["otherActorsUnchanged"]==true,
        "nativeKeyReleaseStopsAllActors":stationary(proof("dedicated-client2"),proof("dedicated-key-released")) && trace("key-released")["runtime"]["captured"]==false,
        "nativeRecaptureTargetsSameClient":focus.is_some() && trace("client2-recaptured")["runtime"]["controlledPlayer"].as_u64()==focus && trace("client2-recaptured")["runtime"]["captured"]==true,
        "onlyClient2ActorMovedBeforeFocusLoss":sideways["allPresent"]==true && sideways["ownerMoved"]==true && sideways["otherActorsUnchanged"]==true,
        "focusLossStopsAllActorsAndCapture":stationary(proof("dedicated-focuslost"),proof("dedicated-focuslost-settled")) && trace("focuslost-settled")["runtime"]["captured"]==false,
        "authoritativeLinkInspectionAvailable":applied["inspectionAvailable"]==true,
        "liveProxyClientIdentitiesMatch":applied["clientIdentitiesMatch"]==true,
        "onlyThirdLiveClientLinkHasAppliedShaping":applied["onlyClient3Shaped"]==true,
        "allThreeLiveProxyDirectionsCarryTraffic":applied["allThreeProxiesCarryTraffic"]==true,
        "configurationIsPolicyOnly":check.configuration["policyOnly"]==true && check.configuration["sceneUnchanged"]==true,
        "authoringSceneUnchanged":source_scene.as_ref()==Some(&check.scene_before) && check.trace.iter().all(|row|row["scene"]==check.scene_before),
        "noAuthoringTransactionsDuringPlay":all_revisions && editor.host.store().revision()==check.before,
        "nativeStopReapsOwnWorld":!world.is_null() && editor.play_world.is_none() && editor.state.playing.is_none()
            && stopped["worlds"].as_array().is_some_and(|rows|rows.iter().all(|row|&row["world"]!=world))
            && stopped["stopping"].as_array().is_some_and(|rows|rows.iter().all(|row|&row["world"]!=world))
    })
}
