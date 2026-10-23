#![allow(clippy::unwrap_used)]
use ara_vfx::*;

#[test]
fn repeatable_cpu_capture_and_partition_conservation() {
    for (effect, tick) in samples::all() {
        let a = CpuSystem::at_tick(&effect, tick, None).unwrap();
        let b = CpuSystem::at_tick(&effect, tick, None).unwrap();
        assert_eq!(a.stats(), b.stats());
        assert_eq!(a.events(), b.events());
        assert!(a.stats().alive > 0, "{}", effect.name);
        for (a, b) in a.emitters().iter().zip(b.emitters()) {
            assert_eq!(a.position_age, b.position_age);
            assert_eq!(a.color, b.color);
            assert_eq!(a.alive, b.alive);
            assert_eq!(a.alive.len() + a.dead.len(), a.position_age.len());
            assert!(a.alive.windows(2).all(|w| w[0] < w[1]));
            assert!(a.dead.windows(2).all(|w| w[0] < w[1]));
        }
    }
}
#[test]
fn event_children_spawn_on_next_tick_and_remain_cpu_authoritative() {
    let effect = samples::rain();
    let mut sim = CpuSystem::new(&effect).unwrap();
    loop {
        sim.step(None).unwrap();
        if sim.events().iter().any(|e| e.trigger == Trigger::Collision) {
            break;
        }
        assert!(sim.stats().tick < 100);
    }
    assert_eq!(sim.emitters()[1].alive.len(), 0);
    let events = sim
        .events()
        .iter()
        .filter(|e| e.trigger == Trigger::Collision)
        .count();
    sim.step(None).unwrap();
    assert_eq!(sim.emitters()[1].alive.len(), events * 3);
}
#[test]
fn low_rates_accumulate_and_capacity_never_overflows() {
    let mut effect = samples::sparks();
    effect.emitters[0].capacity = 3;
    effect.emitters[0].stacks.spawn[0].module = Module::Rate { per_second: 2.5 };
    let demand = (0..600)
        .map(|t| spawn_demand(&effect.emitters[0], t, 60))
        .sum::<u32>();
    assert_eq!(demand, 25);
    effect.emitters[0].stacks.spawn[0].module = Module::Rate { per_second: 10000. };
    let sim = CpuSystem::at_tick(&effect, 10, None).unwrap();
    assert_eq!(sim.stats().alive, 3);
    assert!(sim.stats().dropped_spawns > 0);
}
