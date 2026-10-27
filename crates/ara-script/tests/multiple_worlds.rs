//! Reload queues, module generations and callback contexts belong to individual worlds.
mod support;

use ara_script::abi::{RELOAD_L1, RELOAD_L3};
use serde_json::json;
use support::Live;

const SOURCE: &str = r#"namespace Separate;
/// <summary>World-local simulation state.</summary>
[Component] public partial struct Counter {
    /// <summary>The number of simulated steps.</summary>
    public int Ticks;
    // fields
}
public static partial class Count {
    [Query] static void Run(ref Counter c) => c.Ticks += 1;
}
/// <summary>Exercises the world's callback context alongside its native columns.</summary>
public sealed partial class Lamp : Behaviour {
    /// <summary>Visits in this world only.</summary>
    [Property] public partial int Visits { get; set; }
    /// <summary>The input axis sampled from this world's fixed-tick snapshot.</summary>
    [Property] public partial float Axis { get; set; }
    /// <summary>The authored owner in this world.</summary>
    [Property] public partial ulong Player { get; set; }
    /// <summary>Matching volumes in this world, including components with managed fields.</summary>
    [Property] public partial int Volumes { get; set; }
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick) {
        Visits += 1;
        Get<Light>().Intensity += 1;
        Player = Input.Player(Entity);
        Axis = Input.Axis2(Player, "Move").X;
        Span<Entity> scratch = stackalloc Entity[4];
        Volumes = World.Query<PostProcessVolume>(scratch).Length;
        Navigation.GoTo(Entity, new System.Numerics.Vector3(Axis, 0, 0));
        Movement.Stop(Entity);
        Animation.SetBool(Entity, "Moving", Axis != 0);
    }
}
"#;

#[test]
fn two_worlds_reload_independently_and_the_survivor_can_reload_again() {
    let _clr = support::clr_lock();
    let mut first = Live::start("world-reload-first", &[("Separate", SOURCE, &[])]);
    let mut second = Live::start("world-reload-second", &[("Separate", SOURCE, &[])]);
    let a = first.spawn(&[
        ("Counter", json!({"ticks":10})),
        ("Lamp", json!({})),
        ("Light", json!({"intensity":100})),
        ("PostProcessVolume", json!({})),
    ]);
    let b = second.spawn(&[
        ("Counter", json!({"ticks":1000})),
        ("Lamp", json!({})),
        ("Light", json!({"intensity":200})),
    ]);
    assert_eq!(a, b, "entity handles intentionally alias across worlds");
    inputs(&mut first, a, 11, 0.25);
    inputs(&mut second, b, 22, -0.75);
    first.run();
    second.run();
    callbacks(&mut first, a, 11, 0.25, 1);
    callbacks(&mut second, b, 22, -0.75, 0);
    assert_eq!(first.int(a, "Counter", "ticks"), 11);
    assert_eq!(second.int(b, "Counter", "ticks"), 1001);
    first.edit("Separate", "c.Ticks += 1", "c.Ticks += 3");
    first.until("first world's L1", |s| s.count(RELOAD_L1) > 0);
    let before = first.int(a, "Counter", "ticks");
    first.run();
    assert_eq!(first.int(a, "Counter", "ticks"), before + 3);
    assert_eq!(second.int(b, "Counter", "ticks"), 1001);
    assert_eq!(second.int(b, "Lamp", "visits"), 1);
    assert!(
        second.reports.is_empty(),
        "first world's reports never enter the second queue"
    );
    first.edit_many("Separate", &[
        ("// fields", "/// <summary>Added only in the first world.</summary>\n public int Added;\n // fields"),
        ("c.Ticks += 3", "c.Ticks += 5"),
    ]);
    first.until("first world's L3", |s| s.count(RELOAD_L3) > 0);
    let before = first.int(a, "Counter", "ticks");
    first.run();
    assert_eq!(first.int(a, "Counter", "ticks"), before + 5);
    assert_eq!(first.int(a, "Counter", "added"), 0);
    callbacks(&mut first, a, 11, 0.25, 1);
    assert!(
        second
            .host
            .schema()
            .get("Counter")
            .expect("second schema")
            .field("added")
            .is_none()
    );
    second.run();
    assert_eq!(second.int(b, "Counter", "ticks"), 1002);
    assert_eq!(second.int(b, "Lamp", "visits"), 2);
    callbacks(&mut second, b, 22, -0.75, 0);
    assert_eq!(
        second
            .host
            .read_component(&second.world, b, "Light")
            .expect("light")["intensity"],
        202.0
    );
    assert_eq!(first.finish(), 0);
    second.edit("Separate", "c.Ticks += 1", "c.Ticks += 7");
    second.until("survivor's own L1", |s| s.count(RELOAD_L1) > 0);
    let before = second.int(b, "Counter", "ticks");
    second.run();
    assert_eq!(second.int(b, "Counter", "ticks"), before + 7);
    callbacks(&mut second, b, 22, -0.75, 0);
    assert!(
        second.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        second.diagnostics
    );
    assert_eq!(second.finish(), 0);
}

fn inputs(live: &mut Live, entity: bevy_ecs::entity::Entity, player: u64, axis: f32) {
    live.world.init_resource::<ara_script::ScriptInputs>();
    live.world.init_resource::<ara_script::ScriptNavigation>();
    live.world.init_resource::<ara_script::ScriptControls>();
    let mut inputs = live.world.resource_mut::<ara_script::ScriptInputs>();
    inputs.players.insert(entity.to_bits(), player);
    inputs.actions.insert((player, "Move".into()), [axis, 0.0]);
}

/// Every newly merged callback must still reach its own world after L1/L3 and shutdown.
fn callbacks(
    live: &mut Live,
    entity: bevy_ecs::entity::Entity,
    player: u64,
    axis: f32,
    volumes: i64,
) {
    let lamp = live
        .host
        .read_component(&live.world, entity, "Lamp")
        .expect("lamp");
    assert_eq!(lamp["player"], player);
    assert_eq!(lamp["axis"], axis);
    assert_eq!(lamp["volumes"], volumes);
    let navigation = &live.world.resource::<ara_script::ScriptNavigation>().0;
    assert!(!navigation.is_empty());
    assert!(
        navigation
            .iter()
            .all(|request| *request == (entity.to_bits(), Some([axis, 0.0, 0.0])))
    );
    let controls = live.world.resource::<ara_script::ScriptControls>();
    assert!(!controls.stopped.is_empty());
    assert!(controls.stopped.iter().all(|&e| e == entity.to_bits()));
    assert_eq!(controls.animation.len(), controls.stopped.len());
    assert!(
        controls
            .animation
            .iter()
            .all(|(e, name, value)| *e == entity.to_bits() && name == "Moving" && *value)
    );
    live.world
        .resource_mut::<ara_script::ScriptNavigation>()
        .0
        .clear();
    let mut controls = live.world.resource_mut::<ara_script::ScriptControls>();
    controls.stopped.clear();
    controls.animation.clear();
}
