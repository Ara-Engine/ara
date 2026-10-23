#![allow(clippy::unwrap_used)]
use ara_bench::{Scenario, ScenarioConfig};
use ara_profile::{Config, Counters, Profiler, ThreadRegistration};
struct Work {
    ticks: u32,
    fail: bool,
}
impl Scenario for Work {
    fn reset(&mut self) -> Result<(), String> {
        self.ticks = 0;
        Ok(())
    }
    fn frame(&mut self) -> Result<Counters, String> {
        let _span = ara_profile::scope("work");
        if self.fail {
            return Err("scenario failed".into());
        }
        self.ticks += 1;
        Ok(Counters {
            entities: u64::from(self.ticks),
            ..Default::default()
        })
    }
}
#[test]
fn paired_rounds_reverse_order_reset_and_correlate_ranges() {
    let mut profiler = Profiler::new(Config {
        history: 16,
        ..Default::default()
    })
    .unwrap();
    profiler.register_thread("scenario").attach().unwrap();
    profiler.start();
    let mut a = Work {
        ticks: 0,
        fail: false,
    };
    let mut b = Work {
        ticks: 0,
        fail: false,
    };
    let rounds = ara_bench::interleave(
        &mut a,
        &mut b,
        &mut profiler,
        ScenarioConfig {
            warmup: 2,
            frames: 3,
            rounds: 2,
        },
    )
    .unwrap();
    assert_eq!(
        rounds
            .iter()
            .map(|r| r.variant.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "b", "a"]
    );
    assert_eq!((rounds[0].first_frame, rounds[0].last_frame), (1, 3));
    assert_eq!((a.ticks, b.ticks), (5, 5));
    assert_eq!(profiler.profile().frames.len(), 12);
    assert_eq!(profiler.profile().frames[0].counters.entities, 3);
    a.fail = true;
    assert!(
        ara_bench::interleave(
            &mut a,
            &mut b,
            &mut profiler,
            ScenarioConfig {
                warmup: 0,
                frames: 1,
                rounds: 1
            }
        )
        .is_err()
    );
    assert!(
        profiler.begin_frame().is_ok(),
        "failing scenario must close its frame"
    );
    ThreadRegistration::detach();
}

#[test]
fn simulation_reports_the_registered_system_name() {
    use tracing_subscriber::prelude::*;
    let mut profiler = Profiler::new(Config {
        history: 1,
        ..Default::default()
    })
    .unwrap();
    profiler.register_thread("simulation").attach().unwrap();
    profiler.start();
    let subscriber = tracing_subscriber::registry().with(ara_profile::ProfileLayer);
    tracing::subscriber::with_default(subscriber, || {
        let mut schema = ara_schema::SchemaDb::new();
        ara_doc::register(&mut schema);
        ara_components::register(&mut schema);
        let document = ara_doc::read(
            r#"{"$schema":"ara://schema/scene/1","id":"sc_profile1","records":{}}"#,
            "test.scene",
            &schema,
            None,
        )
        .doc
        .unwrap();
        let world =
            ara_sim::EditWorld::new(ara_sim::SimRegistry::new(schema).unwrap(), document).unwrap();
        let mut simulation =
            ara_sim::Session::local(world, Default::default(), 1, Default::default()).unwrap();
        simulation.add_system("movement", || {}).unwrap();
        profiler.begin_frame().unwrap();
        simulation.step().unwrap();
        profiler
            .end_frame(Default::default(), Default::default())
            .unwrap();
    });
    assert!(
        profiler.profile().frames[0]
            .spans
            .iter()
            .any(|span| profiler.profile().labels[span.label as usize] == "movement")
    );
    ThreadRegistration::detach();
}
