//! Compiled gameplay uses the actual ECS for name lookup and hierarchy.
mod support;

use ara_sim::{Identity, SceneMetadata};
use bevy_ecs::prelude::ChildOf;
use serde_json::json;

#[test]
fn hierarchy_undo_checks_all_parent_cycles_before_restoring_any_entity() {
    let _clr = support::clr_lock();
    let mut live = support::Live::start(
        "parent-undo",
        &[(
            "Game",
            r#"
/// <summary>Moves two scene entities as one reversible gameplay command.</summary>
[Command("move.both")] public static partial class MoveBoth
{
    /// <summary>Queues the two hierarchy edits.</summary>
    public static CommandResult Execute(ref CommandContext ctx)
    {
        ctx.Commands.SetParent(World.Find("A"), World.Find("New"));
        ctx.Commands.SetParent(World.Find("B"), World.Find("New"));
        return CommandResult.Ok;
    }
}
"#,
            &[],
        )],
    );
    let mut named = |name: &str| {
        live.world
            .spawn((
                Identity(format!("e_{name}")),
                SceneMetadata {
                    name: name.into(),
                    order: name.into(),
                },
            ))
            .id()
    };
    let old = named("Old");
    let new = named("New");
    let a = named("A");
    let b = named("B");
    live.world.entity_mut(a).insert(ChildOf(old));
    live.world.entity_mut(b).insert(ChildOf(old));
    let record = live
        .host
        .execute_command(&mut live.world, "move.both", &json!({}), None)
        .expect("move");
    live.world.entity_mut(old).insert(ChildOf(a));
    assert!(
        live.host.undo(&mut live.world, &record).is_err(),
        "undo would create a cycle"
    );
    for child in [a, b] {
        assert_eq!(
            live.world
                .get::<ChildOf>(child)
                .expect("still moved")
                .parent(),
            new,
            "failed undo must be atomic"
        );
    }
}

#[test]
fn destroy_callbacks_run_once_in_handle_order_even_when_the_last_instance_is_removed() {
    let _clr = support::clr_lock();
    let mut live = support::Live::start(
        "destroy-api",
        &[(
            "Game",
            r#"
/// <summary>Authoritative cleanup order, kept in the ECS.</summary>
[Component] public partial struct DestroyTotals
{
    /// <summary>Observed callback entity indices, packed in order.</summary>
    public ulong Sequence;
}
/// <summary>Per-entity lifetime wrapper with no authoritative object fields.</summary>
public sealed partial class Watcher : Behaviour
{
    protected override void OnDestroy()
    {
        ref var totals = ref World.Get<DestroyTotals>(World.Find("Observer"));
        totals.Sequence = totals.Sequence * 100 + Entity.Index;
    }
}
"#,
            &[],
        )],
    );
    let observer = live
        .world
        .spawn((
            Identity("e_observer".into()),
            SceneMetadata {
                name: "Observer".into(),
                order: "a0".into(),
            },
        ))
        .id();
    live.host
        .insert_component(&mut live.world, observer, "DestroyTotals", &json!({}))
        .expect("totals");
    let first = live.world.spawn_empty().id();
    let second = live.world.spawn_empty().id();
    for e in [first, second] {
        live.host
            .insert_component(&mut live.world, e, "Watcher", &json!({}))
            .expect("watcher");
    }
    live.tick();
    live.world.despawn(first);
    let component = live.host.component_id("Watcher").expect("watcher type");
    live.world.entity_mut(second).remove_by_id(component);
    live.tick();
    let expected = u64::from(second.index().index()) * 100 + u64::from(first.index().index());
    assert_eq!(
        live.host
            .read_component(&live.world, observer, "DestroyTotals"),
        Some(json!({"sequence":expected}))
    );
    live.tick();
    assert_eq!(
        live.host
            .read_component(&live.world, observer, "DestroyTotals"),
        Some(json!({"sequence":expected})),
        "destroy is exactly once"
    );
    assert!(live.diagnostics.is_empty(), "{:?}", live.diagnostics);
}

#[test]
fn named_paths_and_hierarchy_read_the_ecs_and_reparent_at_the_sync_point() {
    let _clr = support::clr_lock();
    let mut live = support::Live::start(
        "entity-api",
        &[(
            "Game",
            r#"
/// <summary>Checks scene queries and queues an ECS hierarchy edit.</summary>
public sealed partial class Probe : Behaviour
{
    /// <summary>Number of successful checks.</summary>
    [Property] public partial int Checks { get; set; }
    protected override void OnTick(in Tick tick)
    {
        if (Checks != 0) return;
        var root = World.Find("Root");
        var child = World.Find("Root/Child");
        Span<Entity> scratch = stackalloc Entity[4];
        if (root.IsAlive && child.IsAlive && World.Find("missing").Equals(Entity.Null)
            && World.Parent(child).Equals(root) && World.Children(root, scratch).Length == 2
            && World.Siblings(child, scratch).Length == 1 && World.Find("e_child").Equals(child)) Checks = 7;
        try { World.Find("Duplicate"); } catch (System.InvalidOperationException) { Checks++; }
        try { World.Children(root, Span<Entity>.Empty); } catch (System.ArgumentException) { Checks++; }
        Commands.SetParent(child, Entity);
    }
}
"#,
            &[],
        )],
    );
    let named = |name: &str| SceneMetadata {
        name: name.into(),
        order: name.into(),
    };
    let root = live
        .world
        .spawn((Identity("e_root".into()), named("Root")))
        .id();
    let child = live
        .world
        .spawn((Identity("e_child".into()), named("Child"), ChildOf(root)))
        .id();
    live.world
        .spawn((Identity("e_other".into()), named("Other"), ChildOf(root)));
    live.world
        .spawn((Identity("e_alias".into()), named("e_child")));
    live.world
        .spawn((Identity("e_dup1".into()), named("Duplicate")));
    live.world
        .spawn((Identity("e_dup2".into()), named("Duplicate")));
    let probe = live
        .world
        .spawn((Identity("e_probe".into()), named("Probe")))
        .id();
    live.host
        .insert_component(&mut live.world, probe, "Probe", &json!({}))
        .expect("probe");
    live.tick();
    assert_eq!(
        live.host.read_component(&live.world, probe, "Probe"),
        Some(json!({"checks":9}))
    );
    assert_eq!(
        live.world
            .get::<ChildOf>(child)
            .expect("reparented")
            .parent(),
        probe
    );
    assert!(live.host.take_diagnostics().is_empty());
}
