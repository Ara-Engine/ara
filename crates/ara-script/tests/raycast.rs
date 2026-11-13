//! `Physics.Raycast` reaches the embedding runtime's ray query and brings its answer back.
mod support;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use serde_json::json;
use support::Live;

const SOURCE: &str = r#"namespace Rays;
/// <summary>Looks down from a fixed point each tick and records what the ray found.</summary>
public sealed partial class Probe : Behaviour {
    /// <summary>Whether the ray hit.</summary>
    [Property] public partial bool Hit { get; set; }
    /// <summary>The hit entity's handle.</summary>
    [Property] public partial ulong Under { get; set; }
    /// <summary>Distance to the hit in metres.</summary>
    [Property] public partial float Distance { get; set; }
    /// <summary>The hit point.</summary>
    [Property] public partial System.Numerics.Vector3 Point { get; set; }
    /// <summary>The surface normal.</summary>
    [Property] public partial System.Numerics.Vector3 Normal { get; set; }
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick) {
        // Not unit length on purpose: the direction is normalised, the distance is metres.
        Hit = Physics.Raycast(new System.Numerics.Vector3(1, 10, 2), new System.Numerics.Vector3(0, -4, 0), 20f, out var hit, Entity);
        Under = hit.Entity.Bits;
        Distance = hit.Distance;
        Point = hit.Point;
        Normal = hit.Normal;
    }
}
"#;

/// A floor at y = 3 that reports the ignored entity as its owner, so the test sees every
/// argument arrive: origin, the full displacement and `ignore`.
unsafe fn floor(
    _: UnsafeWorldCell<'_>,
    origin: [f32; 3],
    displacement: [f32; 3],
    ignore: Option<Entity>,
) -> Option<ara_script::ScriptRayHit> {
    let fraction = (origin[1] - 3.0) / -displacement[1];
    (0.0..=1.0)
        .contains(&fraction)
        .then(|| ara_script::ScriptRayHit {
            entity: ignore.map_or(0, Entity::to_bits),
            point: [origin[0], 3.0, origin[2]],
            normal: [0.0, 1.0, 0.0],
            fraction,
        })
}

#[test]
fn a_behaviour_casts_a_ray_through_the_runtimes_query() {
    let _clr = support::clr_lock();
    let mut live = Live::start("raycast", &[("Rays", SOURCE, &[])]);
    let probe = live.spawn(&[("Probe", json!({}))]);
    let read = |live: &Live| {
        live.host
            .read_component(&live.world, probe, "Probe")
            .expect("probe")
    };

    // No runtime query: every ray misses, nothing throws.
    live.run();
    assert_eq!(read(&live)["hit"], false);

    live.world.insert_resource(ara_script::ScriptRays(floor));
    live.run();
    let seen = read(&live);
    assert_eq!(seen["hit"], true);
    assert_eq!(seen["under"], probe.to_bits(), "`ignore` reached the query");
    assert_eq!(seen["distance"], 7.0, "0.35 of a 20 m ray");
    assert_eq!(seen["point"], json!([1.0, 3.0, 2.0]));
    assert_eq!(seen["normal"], json!([0.0, 1.0, 0.0]));
    assert!(
        live.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        live.diagnostics
    );
    assert_eq!(live.finish(), 0);
}
