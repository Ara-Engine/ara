//! `Physics.Contacts` reaches the embedding runtime's contact query and brings its list back.
mod support;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use serde_json::json;
use support::Live;

const SOURCE: &str = r#"namespace Touch;
/// <summary>Records what the entity touched this tick.</summary>
public sealed partial class Feeler : Behaviour {
    /// <summary>How many contacts there were.</summary>
    [Property] public partial int Count { get; set; }
    /// <summary>The first contact's other entity.</summary>
    [Property] public partial ulong Other { get; set; }
    /// <summary>The first contact's normal.</summary>
    [Property] public partial System.Numerics.Vector3 Normal { get; set; }
    /// <summary>The first contact's point.</summary>
    [Property] public partial System.Numerics.Vector3 Point { get; set; }
    /// <summary>The first contact's phase.</summary>
    [Property] public partial int Phase { get; set; }
    /// <summary>Whether the last contact is a trigger.</summary>
    [Property] public partial bool LastIsTrigger { get; set; }
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick) {
        Span<Contact> scratch = stackalloc Contact[8];
        var contacts = Physics.Contacts(Entity, scratch);
        Count = contacts.Length;
        if (Count == 0) return;
        Other = contacts[0].Other.Bits;
        Normal = contacts[0].Normal;
        Point = contacts[0].Point;
        Phase = (int)contacts[0].Phase;
        LastIsTrigger = contacts[^1].IsTrigger;
    }
}
"#;

/// Two contacts for whoever asks: a persisted solid one with the asker as `other` (so the test
/// sees the entity argument arrive), then a trigger that just started.
unsafe fn two(_: UnsafeWorldCell<'_>, entity: Entity, out: &mut Vec<ara_script::ScriptContact>) {
    out.push(ara_script::ScriptContact {
        other: entity.to_bits(),
        point: [1.0, 2.0, 3.0],
        normal: [0.0, 1.0, 0.0],
        phase: 1,
        sensor: false,
    });
    out.push(ara_script::ScriptContact {
        other: 0,
        point: [0.0; 3],
        normal: [1.0, 0.0, 0.0],
        phase: 0,
        sensor: true,
    });
}

#[test]
fn a_behaviour_reads_its_contacts_through_the_runtimes_query() {
    let _clr = support::clr_lock();
    let mut live = Live::start("contacts", &[("Touch", SOURCE, &[])]);
    let feeler = live.spawn(&[("Feeler", json!({}))]);
    let read = |live: &Live| {
        live.host
            .read_component(&live.world, feeler, "Feeler")
            .expect("feeler")
    };

    // No runtime query: nothing touches anything, nothing throws.
    live.run();
    assert_eq!(read(&live)["count"], 0);

    live.world.insert_resource(ara_script::ScriptContacts(two));
    live.run();
    let seen = read(&live);
    assert_eq!(seen["count"], 2);
    assert_eq!(
        seen["other"],
        feeler.to_bits(),
        "the entity reached the query"
    );
    assert_eq!(seen["point"], json!([1.0, 2.0, 3.0]));
    assert_eq!(seen["normal"], json!([0.0, 1.0, 0.0]));
    assert_eq!(seen["phase"], 1, "persisted");
    assert_eq!(seen["lastIsTrigger"], true);
    assert!(
        live.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        live.diagnostics
    );
    assert_eq!(live.finish(), 0);
}
