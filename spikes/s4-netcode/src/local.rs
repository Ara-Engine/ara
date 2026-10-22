//! Local sessions allocate only one ECS world. No codec or transport is constructed.

#[cfg(any(feature = "network", test))]
use crate::gameplay::State;
use crate::gameplay::{self, Frame, Motor, Simulation};
use bevy_ecs::prelude::*;
use serde::Serialize;
use std::{hint::black_box, time::Instant};

pub struct LocalSession {
    pub simulation: Simulation,
}

impl LocalSession {
    pub fn new() -> Self {
        Self {
            simulation: Simulation::new(),
        }
    }
    pub fn step(&mut self, frame: &Frame) {
        self.simulation.step(frame);
    }
}

#[derive(Serialize)]
pub struct LocalReport {
    pub ticks: u32,
    pub state_hash: String,
    pub gameplay_bytes_serialized: u64,
    pub ecs_resources: usize,
    pub baseline_ecs_resources: usize,
    pub network_resources: usize,
    pub transport_objects: usize,
    pub prediction_history_entries: usize,
    pub accepted_commands: u32,
    pub rejected_commands: u32,
}

pub fn run(ticks: u32) -> Result<LocalReport, String> {
    let mut local = LocalSession::new();
    let mut accepted = 0;
    let mut rejected = 0;
    for tick in 1..=ticks {
        let outcome = local.simulation.step(&gameplay::scripted_frame(tick, 4));
        accepted += outcome.accepted;
        rejected += outcome.rejected;
    }
    let state = local.simulation.capture();
    if ticks >= 420
        && (!state.door.open
            || state.motors != [Motor::default(); 4]
            || state
                .inventories
                .iter()
                .enumerate()
                .any(|(id, inventory)| inventory.items != 1 << id))
    {
        return Err("Local scripted outcome is incorrect".into());
    }
    let resources = local.simulation.world.iter_resources().count();
    let baseline_resources = World::new().iter_resources().count();
    if resources != baseline_resources {
        return Err(format!(
            "Local allocated {} resources beyond a plain ECS World",
            resources - baseline_resources
        ));
    }
    Ok(LocalReport {
        ticks,
        state_hash: format!("{:016x}", gameplay::hash(&state)),
        gameplay_bytes_serialized: 0,
        ecs_resources: resources,
        baseline_ecs_resources: baseline_resources,
        network_resources: 0,
        transport_objects: 0,
        prediction_history_entries: 0,
        accepted_commands: accepted,
        rejected_commands: rejected,
    })
}

/// Replay accepted tick records in a fresh world and compare every recorded checkpoint.
#[cfg(any(feature = "network", test))]
pub fn replay(frames: &[Frame], expected: &[State]) -> Result<u64, String> {
    if frames.len() != expected.len() {
        return Err("Replay frame/checkpoint counts differ".into());
    }
    let mut simulation = Simulation::new();
    for (frame, expected) in frames.iter().zip(expected) {
        simulation.step(frame);
        let actual = simulation.capture();
        if let Some(difference) = gameplay::first_difference(expected, &actual) {
            return Err(difference);
        }
    }
    Ok(gameplay::hash(&simulation.capture()))
}

#[derive(Serialize)]
pub struct Benchmark {
    pub iterations_per_sample: u32,
    pub samples: u32,
    pub plain_ns_per_tick: Vec<f64>,
    pub local_ns_per_tick: Vec<f64>,
    pub direct_simulation_ns_per_tick: Vec<f64>,
    pub median_ratio: f64,
    pub median_local_to_direct_ratio: f64,
    pub final_motor: Motor,
}

/// Same four motors, same function, same indexed ECS access; construction excluded.
/// Pair order alternates; all game state is black-boxed to retain actual work.
pub fn benchmark() -> Benchmark {
    let count = 200_000;
    let mut plain = World::new();
    let ids: [Entity; 4] = std::array::from_fn(|_| plain.spawn(Motor::default()).id());
    let mut local = LocalSession::new();
    let mut direct = Simulation::new();
    let mut plain_samples = Vec::new();
    let mut local_samples = Vec::new();
    let mut direct_samples = Vec::new();
    let mut plain_run = || {
        let start = Instant::now();
        for _ in 0..count {
            for entity in ids {
                let mut motor = plain
                    .get_mut::<Motor>(entity)
                    .expect("benchmark motor exists");
                gameplay::step_motor(&mut motor, black_box(gameplay::MoveInput { x: 1, z: 0 }));
            }
        }
        black_box(plain.get::<Motor>(ids[0]));
        start.elapsed().as_nanos() as f64 / f64::from(count)
    };
    let mut tick = 0;
    let mut frame = Frame {
        tick,
        inputs: [gameplay::MoveInput { x: 1, z: 0 }; 4],
        commands: Vec::new(),
    };
    let mut local_run = || {
        let start = Instant::now();
        for _ in 0..count {
            tick += 1;
            frame.tick = tick;
            local.step(black_box(&frame));
        }
        black_box(local.simulation.motor(0));
        start.elapsed().as_nanos() as f64 / f64::from(count)
    };
    let mut direct_tick = 0;
    let mut direct_frame = Frame {
        tick: 0,
        inputs: [gameplay::MoveInput { x: 1, z: 0 }; 4],
        commands: Vec::new(),
    };
    let mut direct_run = || {
        let start = Instant::now();
        for _ in 0..count {
            direct_tick += 1;
            direct_frame.tick = direct_tick;
            direct.step(black_box(&direct_frame));
        }
        black_box(direct.motor(0));
        start.elapsed().as_nanos() as f64 / f64::from(count)
    };
    for sample in 0..9 {
        let (p, l) = if sample % 2 == 0 {
            (plain_run(), local_run())
        } else {
            let l = local_run();
            (plain_run(), l)
        };
        let d = direct_run();
        if sample > 0 {
            plain_samples.push(p);
            local_samples.push(l);
            direct_samples.push(d);
        }
    }
    let median = |values: &[f64]| {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        (sorted[3] + sorted[4]) / 2.0
    };
    Benchmark {
        iterations_per_sample: count,
        samples: 8,
        median_ratio: median(&local_samples) / median(&plain_samples),
        median_local_to_direct_ratio: median(&local_samples) / median(&direct_samples),
        direct_simulation_ns_per_tick: direct_samples,
        plain_ns_per_tick: plain_samples,
        local_ns_per_tick: local_samples,
        final_motor: local.simulation.motor(0),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::gameplay::{Command, Intent, MoveInput, ToggleDoor};

    #[test]
    fn local_has_no_resources_and_expected_intent_outcomes() {
        let report = run(600).unwrap();
        assert_eq!(report.ecs_resources, report.baseline_ecs_resources);
        assert_eq!(report.network_resources, 0);
        assert_eq!(report.accepted_commands, 5);
        assert_eq!(report.rejected_commands, 1);
    }

    #[test]
    fn replay_compares_every_tick_and_reports_the_first_field() {
        let mut simulation = Simulation::new();
        let mut frames = Vec::new();
        let mut states = Vec::new();
        for tick in 1..=240 {
            let frame = gameplay::scripted_frame(tick, 4);
            simulation.step(&frame);
            frames.push(frame);
            states.push(simulation.capture());
        }
        assert_eq!(
            replay(&frames, &states).unwrap(),
            gameplay::hash(states.last().unwrap())
        );
        states[120].motors[2].x += 1;
        assert_eq!(
            replay(&frames, &states).unwrap_err(),
            "tick 121 player 2.motor.x: 551 != 550"
        );
        states[120].motors[2].x -= 1;
        states[120].door.changed += 1;
        assert_eq!(
            replay(&frames, &states).unwrap_err(),
            "tick 121 door.changed"
        );
    }

    #[test]
    fn server_validation_rejects_duplicates_spam_invalid_caller_and_axes() {
        let mut simulation = Simulation::new();
        let command = Command {
            caller: 0,
            sequence: 1,
            intent: Intent::Toggle(ToggleDoor { target: 4 }),
        };
        let frame = Frame {
            tick: 1,
            inputs: [MoveInput { x: i8::MIN, z: 2 }; 4],
            commands: vec![
                command,
                command,
                Command {
                    caller: 255,
                    ..command
                },
            ],
        };
        let result = simulation.step(&frame);
        assert_eq!((result.accepted, result.rejected), (1, 2));
        assert_eq!(simulation.motor(0), Motor::default());
        let frame = Frame {
            tick: 2,
            inputs: [MoveInput::default(); 4],
            commands: vec![Command {
                sequence: 2,
                ..command
            }],
        };
        assert_eq!(simulation.step(&frame).rejected, 1);
        assert!(simulation.capture().door.open);
    }
}
