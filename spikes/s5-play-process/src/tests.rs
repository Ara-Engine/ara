//! Focused tests for the pieces both processes rely on: snapshot fidelity, op inverses and
//! atomicity, stable-ID ordering and tick determinism.

use serde_json::json;

use crate::model::{SceneId, Sim, state_hash};
use crate::registry::{Op, apply_ops, read_path};
use crate::scene::{SceneDoc, generate};
use crate::snapshot;

fn world(count: usize) -> bevy_ecs::world::World {
    generate(count, 7)
        .into_world()
        .expect("generated scenes are valid")
}

#[test]
fn snapshots_preserve_state_bit_for_bit() -> anyhow::Result<()> {
    let mut edit = world(500);
    let hash = state_hash(&mut edit);
    let mut from_binary = snapshot::decode(&snapshot::encode(&mut edit))?;
    assert_eq!(state_hash(&mut from_binary), hash);
    let json = serde_json::to_vec(&SceneDoc::from_world(&mut edit))?;
    let mut from_json = serde_json::from_slice::<SceneDoc>(&json)?.into_world()?;
    assert_eq!(state_hash(&mut from_json), hash);
    // Equal worlds encode to identical bytes.
    assert_eq!(
        snapshot::encode(&mut from_binary),
        snapshot::encode(&mut edit)
    );
    Ok(())
}

#[test]
fn op_inverses_restore_the_world() -> anyhow::Result<()> {
    let mut w = world(50);
    let before = state_hash(&mut w);
    let ops = vec![
        Op::Set {
            target: SceneId(2),
            path: "Orbit.radius".into(),
            value: json!(4.5),
            prev: None,
        },
        Op::Set {
            target: SceneId(3),
            path: "Tint".into(),
            value: json!({ "rgba": [1.0, 1.0, 1.0, 1.0] }),
            prev: None,
        },
        Op::Remove {
            target: SceneId(4),
            component: "Orbit".into(),
            prev: None,
        },
        Op::Create {
            id: None,
            components: [("Label".to_owned(), json!("new"))].into(),
        },
        Op::Delete {
            target: SceneId(5),
            snapshot: None,
        },
    ];
    let inverse = apply_ops(&mut w, &ops)?;
    assert_eq!(read_path(&w, SceneId(2), "Orbit.radius"), Some(json!(4.5)));
    assert_eq!(read_path(&w, SceneId(4), "Orbit"), None);
    apply_ops(&mut w, &inverse)?;
    assert_eq!(state_hash(&mut w), before);
    Ok(())
}

#[test]
fn a_failing_op_applies_nothing() {
    let mut w = world(20);
    let before = state_hash(&mut w);
    let ops = vec![
        Op::Set {
            target: SceneId(2),
            path: "Orbit.radius".into(),
            value: json!(4.5),
            prev: None,
        },
        Op::Set {
            target: SceneId(3),
            path: "Orbit.radius".into(),
            value: json!("wide"),
            prev: None,
        },
    ];
    let err = apply_ops(&mut w, &ops).expect_err("a string radius must be rejected");
    assert!(format!("{err:#}").contains("op 1"), "{err:#}");
    assert_eq!(state_hash(&mut w), before);
}

#[test]
fn scene_id_keys_sort_numerically() -> anyhow::Result<()> {
    let ids = [
        SceneId(1),
        SceneId(35),
        SceneId(36),
        SceneId(1295),
        SceneId(1296),
        SceneId(50_000),
    ];
    let keys: Vec<String> = ids.iter().map(|id| id.key()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    for (id, key) in ids.iter().zip(&keys) {
        assert_eq!(SceneId::parse(key)?, *id);
    }
    Ok(())
}

#[test]
fn ticking_is_deterministic() -> anyhow::Result<()> {
    let bytes = snapshot::encode(&mut world(300));
    let run = || -> anyhow::Result<u64> {
        let mut sim = Sim::new(snapshot::decode(&bytes)?);
        for _ in 0..120 {
            sim.tick();
        }
        Ok(state_hash(&mut sim.world))
    };
    assert_eq!(run()?, run()?);
    Ok(())
}
