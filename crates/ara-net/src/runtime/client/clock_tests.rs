//! Device scheduling observes corrections applied after the input sampling schedule.
use super::*;

/// Complete state for the input-clock fixture.
#[derive(Component, Schema, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[ara(component)]
struct ClockState {
    /// Fixture position.
    #[ara(replicate(audience=Relevant,rollback))]
    x: i32,
}
struct ClockFeature;
impl SimulationFeature for ClockFeature {
    type State = ClockState;
    type Input = i8;
    const ACTION: &'static str = "Move";
    fn input(_: Option<ActionValue>) -> std::result::Result<i8, ara_sim::SimError> {
        Ok(0)
    }
    fn action(_: &i8) -> std::result::Result<ActionValue, ara_sim::SimError> {
        Ok(ActionValue::Axis2([0.; 2]))
    }
    fn step(_: &mut ClockState, _: &i8) {}
    fn interpolate(_: ClockState, b: ClockState, _: f32) -> ClockState {
        b
    }
    fn correction(_: &ClockState, _: &ClockState) -> f64 {
        0.
    }
}

fn client(sampled: u64) -> Client<ClockFeature> {
    let mut app = App::new();
    app.insert_resource(LocalTimeline::default());
    app.insert_resource(LocalTimelineSync::default());
    app.insert_resource(PredictionMetrics::default());
    app.insert_resource(Owner {
        player: PlayerId(1),
        origin: Some(0),
        ready: true,
        limit: None,
        inputs: BTreeMap::new(),
        pending_actions: BTreeMap::new(),
        source: None,
        last_checked: BTreeMap::new(),
        status: ClientStatus {
            predicted_tick: sampled,
            ..Default::default()
        },
        fault: None,
        references: crate::DeferredReferences::new(4096),
        visible: BTreeSet::new(),
        field_views: BTreeMap::new(),
    });
    Client {
        app,
        marker: PhantomData,
        next_update: std::time::Instant::now(),
        poll_interval: Duration::from_secs_f64(1. / 60.),
    }
}

#[test]
fn device_tick_follows_a_clock_shift_after_sampling() {
    let mut client = client(5);
    client
        .app
        .world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta(1000);
    let expected = u64::from(client.app.world().resource::<LocalTimeline>().tick().0);
    assert_eq!(
        client.status().predicted_tick,
        expected,
        "PostUpdate correction must be visible to device callers"
    );
}

#[test]
fn a_backward_clock_correction_accepts_the_actual_next_input_tick() -> Result<()> {
    let mut client = client(1000);
    client
        .app
        .world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta(100);
    let next = u64::from(client.app.world().resource::<LocalTimeline>().tick().0) + 1;
    client.inject(
        next,
        Injection {
            player: PlayerId(1),
            action: "Move".into(),
            value: ActionValue::Axis2([1., 0.]),
            hold_ticks: 1,
        },
    )?;
    assert!(
        client
            .app
            .world()
            .resource::<Owner>()
            .inputs
            .contains_key(&(next, "Move".into()))
    );
    Ok(())
}

#[test]
fn immediate_one_tick_press_is_stamped_when_sampling_after_a_clock_shift() -> Result<()> {
    use bevy_ecs::system::RunSystemOnce;
    let mut client = client(5);
    client
        .app
        .world_mut()
        .resource_mut::<LocalTimelineSync>()
        .set_synced(true);
    client.inject_next(Injection {
        player: PlayerId(1),
        action: "Move".into(),
        value: ActionValue::Axis2([1., 0.]),
        hold_ticks: 1,
    })?;
    client
        .app
        .world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta(1000);
    let expected = u64::from(client.app.world().resource::<LocalTimeline>().tick().0);
    client
        .app
        .world_mut()
        .run_system_once(systems::inputs::<ClockFeature>)
        .expect("input sampling");
    let owner = client.app.world().resource::<Owner>();
    assert!(owner.pending_actions.is_empty());
    assert_eq!(
        owner
            .inputs
            .get(&(expected, "Move".into()))
            .map(|i| i.hold_ticks),
        Some(1)
    );
    Ok(())
}
