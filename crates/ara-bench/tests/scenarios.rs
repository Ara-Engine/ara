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
