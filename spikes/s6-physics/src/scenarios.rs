//! The S6 scenarios, written once against `Engine` so both backends get identical scenes,
//! inputs and metrics. Every run is fixed-step at 60 Hz.

use crate::common::{DT, Fnv, Lcg, Stats, Vec3, ms_since};
use crate::engine::*;
use crate::scene;
use serde::Serialize;
use std::time::Instant;

fn v3(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

// ---------------------------------------------------------------------------------------
// (a) Character controller
// ---------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct LaneResult {
    pub lane: &'static str,
    pub pass: bool,
    pub expected: String,
    pub measured: String,
    pub final_feet: [f32; 3],
}

#[derive(Serialize)]
pub struct CharacterReport {
    pub engine: &'static str,
    pub lanes: Vec<LaneResult>,
    pub update_us_per_character: Stats,
}

/// Platform paths: +/-3 m along Z (up to 4.7 m/s), optionally +/-0.5 m in Y as well.
fn platform_path(x: f32, vertical: bool, t: f32) -> Vec3 {
    use std::f32::consts::TAU;
    let y = if vertical {
        1.0 + 0.5 * (TAU * t / 3.0).sin()
    } else {
        1.0
    };
    Vec3::new(x, y, 6.0 + 3.0 * (TAU * t / 4.0).sin())
}

pub fn character<E: Engine>() -> CharacterReport {
    let mut e = E::new(WorldConfig {
        max_bodies: 256,
        threads: 1,
    });
    scene::ground(&mut e, 60.0);
    let h35 = scene::ramp(&mut e, -22.0, 0.0, 35.0, 8.0, 10.0);
    scene::ramp(&mut e, -16.0, 0.0, 35.0, 8.0, 10.0);
    scene::ramp(&mut e, -10.0, 0.0, 50.0, 6.0, 10.0);
    let stair_top = scene::stairs(&mut e, -4.0, 0.0, 6, 0.3, 0.4);
    let platform_half = Vec3::new(1.5, 0.1, 1.5);
    let plat_h = |t| platform_path(2.0, false, t);
    let plat_v = |t| platform_path(6.0, true, t);
    let platforms = [
        e.add_kinematic_box(plat_h(0.0), platform_half),
        e.add_kinematic_box(plat_v(0.0), platform_half),
    ];
    let wall_face = 3.75;
    e.add_static_box(
        Vec3::new(16.0, 1.5, wall_face + 0.25),
        crate::common::Quat::IDENTITY,
        Vec3::new(4.0, 1.5, 0.25),
    );
    e.finish_setup();

    let desc = |p: Vec3| CharacterDesc {
        position: p,
        radius: 0.3,
        half_height: 0.6,
        max_slope_deg: 45.0,
        step_height: 0.35,
        stick_down: 0.5,
        padding: 0.02,
    };
    // (start, walk velocity, walk ticks); the rest of the 6 s run is idle.
    let lanes = [
        (Vec3::new(-22.0, 0.0, -2.0), Vec3::new(0.0, 0.0, 3.0), 300),
        (Vec3::new(-16.0, 0.0, -2.0), Vec3::new(0.0, 0.0, 3.0), 120),
        (Vec3::new(-10.0, 0.0, -2.0), Vec3::new(0.0, 0.0, 3.0), 360),
        (Vec3::new(-4.0, 0.0, -2.0), Vec3::new(0.0, 0.0, 2.0), 240),
        (plat_h(0.0) + Vec3::new(0.0, 0.15, 0.0), Vec3::ZERO, 0),
        (plat_v(0.0) + Vec3::new(0.0, 0.15, 0.0), Vec3::ZERO, 0),
        (
            Vec3::new(13.0, 0.0, 2.0),
            Vec3::new(1.0, 0.0, 1.0).normalize() * 3.0,
            180,
        ),
    ];
    let ids: Vec<u32> = lanes
        .iter()
        .map(|(p, _, _)| e.add_character(&desc(*p)))
        .collect();

    let ticks = 360; // 6 s
    let mut max_y50 = 0.0f32;
    let mut stand_y_at_stop = 0.0f32;
    let mut max_wall_penetration = f32::MIN;
    let mut platform_drift_max = [0.0f32; 2];
    let mut platform_gap_max = [0.0f32; 2];
    let mut platform_ref = [None, None];
    let mut update_samples = Vec::with_capacity(ticks);
    let mut platform_state = Vec::new();
    for tick in 0..ticks {
        let t_next = (tick + 1) as f32 * DT;
        e.move_kinematic(platforms[0], plat_h(t_next));
        e.move_kinematic(platforms[1], plat_v(t_next));
        let start = Instant::now();
        let mut states = Vec::with_capacity(ids.len());
        for (id, (_, vel, walk)) in ids.iter().zip(&lanes) {
            let vel = if tick < *walk { *vel } else { Vec3::ZERO };
            states.push(e.update_character(*id, vel));
        }
        update_samples.push(ms_since(start) * 1000.0 / ids.len() as f64);
        e.step();

        if tick + 1 == lanes[1].2 {
            stand_y_at_stop = states[1].position.y;
        }
        max_y50 = max_y50.max(states[2].position.y);
        max_wall_penetration = max_wall_penetration.max(states[6].position.z + 0.3 - wall_face);
        e.read(&platforms, &mut platform_state);
        for k in 0..2 {
            let rel = states[4 + k].position - platform_state[k].position;
            let horiz = Vec3::new(rel.x, 0.0, rel.z);
            // Reference offset once the rider has landed (0.5 s); drift is measured against it.
            if tick >= 30 {
                let r = *platform_ref[k].get_or_insert(horiz);
                platform_drift_max[k] = platform_drift_max[k].max((horiz - r).length());
                platform_gap_max[k] = platform_gap_max[k].max((rel.y - platform_half.y).abs());
            }
        }
    }

    let fin: Vec<CharacterState> = ids.iter().map(|id| e.character_state(*id)).collect();
    let stand_slide = stand_y_at_stop - fin[1].position.y;
    let wall_slide = fin[6].position.x - lanes[6].0.x;
    let lane = |lane, pass, expected: String, measured: String, i: usize| LaneResult {
        lane,
        pass,
        expected,
        measured,
        final_feet: v3(fin[i].position),
    };
    let results = vec![
        lane(
            "slope_35deg_climb",
            (fin[0].position.y - h35).abs() < 0.05,
            format!("walk up 8 m of 35 deg slope to the landing at y = {h35:.3}"),
            format!("final y = {:.3}", fin[0].position.y),
            0,
        ),
        lane(
            "slope_35deg_idle",
            stand_y_at_stop > 1.0 && stand_slide.abs() < 0.02,
            "stop half-way up the 35 deg slope and stand still for 4 s".into(),
            format!("stopped at y = {stand_y_at_stop:.3}, slid {stand_slide:.4} m while idle"),
            1,
        ),
        lane(
            "slope_50deg_control",
            max_y50 < 0.6,
            "blocked: a 50 deg slope exceeds the 45 deg limit".into(),
            format!("max y = {max_y50:.3}"),
            2,
        ),
        lane(
            "stairs_0.3m",
            (fin[3].position.y - stair_top).abs() < 0.05,
            format!("climb 6 x 0.3 m steps (0.4 m treads) to y = {stair_top:.2}"),
            format!("final y = {:.3}", fin[3].position.y),
            3,
        ),
        lane(
            "moving_platform_horizontal",
            platform_drift_max[0] < 0.15 && platform_gap_max[0] < 0.05,
            "stand on a platform moving +/-3 m along Z (up to 4.7 m/s) for 6 s".into(),
            format!(
                "max drift vs platform = {:.3} m, max |feet - top| = {:.3} m",
                platform_drift_max[0], platform_gap_max[0]
            ),
            4,
        ),
        lane(
            "moving_platform_vertical",
            platform_drift_max[1] < 0.15 && platform_gap_max[1] < 0.05,
            "same, plus +/-0.5 m vertical bob (up to 1.05 m/s)".into(),
            format!(
                "max drift vs platform = {:.3} m, max |feet - top| = {:.3} m",
                platform_drift_max[1], platform_gap_max[1]
            ),
            5,
        ),
        lane(
            "wall_slide",
            max_wall_penetration < 0.01 && wall_slide > 3.0,
            "walk 45 deg into a wall for 3 s: no penetration, slide along it".into(),
            format!(
                "max capsule penetration = {:.4} m, slide along wall = {wall_slide:.2} m",
                max_wall_penetration.max(0.0)
            ),
            6,
        ),
    ];
    CharacterReport {
        engine: E::NAME,
        lanes: results,
        update_us_per_character: Stats::from_ms(&update_samples),
    }
}

// ---------------------------------------------------------------------------------------
// (b) Stacking
// ---------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct StackReport {
    pub engine: &'static str,
    /// None = engine defaults.
    pub solver_iterations: Option<u32>,
    pub pyramid_boxes: usize,
    pub tower_boxes: usize,
    /// Largest horizontal (XZ) displacement of any box after 10 s.
    pub pyramid_max_lean_10s: f32,
    pub pyramid_max_sag_10s: f32,
    pub tower_max_lean_10s: f32,
    /// Top box height at 10 s (19.5 when placed).
    pub tower_top_height_10s: f32,
    pub standing_10s: bool,
    pub all_asleep_at_s: Option<f32>,
    pub active_at_10s: usize,
    /// First second, while every body is awake.
    pub step_ms_first_second: Stats,
    pub step_ms: Stats,
}

pub fn stacking<E: Engine>(solver_iterations: Option<u32>) -> StackReport {
    let mut e = E::new(WorldConfig {
        max_bodies: 1024,
        threads: 1,
    });
    scene::ground(&mut e, 60.0);
    let pyr = scene::pyramid(10, Vec3::ZERO);
    let tow = scene::tower(20, Vec3::new(15.0, 0.0, 0.0));
    let pyr_ids = e.add_dynamic(&pyr);
    let tow_ids = e.add_dynamic(&tow);
    e.finish_setup();
    if let Some(n) = solver_iterations {
        e.set_solver_iterations(n);
    }

    let lean = |s: &BodyState, d: &DynamicDesc| {
        let v = s.position - d.position;
        Vec3::new(v.x, 0.0, v.z).length()
    };
    let mut steps = Vec::new();
    let mut asleep_at = None;
    let mut state = Vec::new();
    let (mut p_lean, mut p_sag, mut t_lean, mut top, mut active10) = (0.0, 0.0, 0.0, 0.0, 0);
    for tick in 1..=1800 {
        let start = Instant::now();
        e.step();
        steps.push(ms_since(start));
        if asleep_at.is_none() && e.active_count() == 0 {
            asleep_at = Some(tick as f32 * DT);
        }
        if tick == 600 {
            active10 = e.active_count();
            e.read(&pyr_ids, &mut state);
            p_lean = state
                .iter()
                .zip(&pyr)
                .map(|(s, d)| lean(s, d))
                .fold(0.0, f32::max);
            p_sag = state
                .iter()
                .zip(&pyr)
                .map(|(s, d)| d.position.y - s.position.y)
                .fold(0.0, f32::max);
            e.read(&tow_ids, &mut state);
            t_lean = state
                .iter()
                .zip(&tow)
                .map(|(s, d)| lean(s, d))
                .fold(0.0, f32::max);
            top = state.last().map_or(0.0, |s| s.position.y);
        }
        if tick >= 600 && asleep_at.is_some() {
            break;
        }
    }
    StackReport {
        engine: E::NAME,
        solver_iterations,
        pyramid_boxes: pyr.len(),
        tower_boxes: tow.len(),
        pyramid_max_lean_10s: p_lean,
        pyramid_max_sag_10s: p_sag,
        tower_max_lean_10s: t_lean,
        tower_top_height_10s: top,
        standing_10s: p_lean < 0.1 && t_lean < 0.1 && top > 19.0,
        all_asleep_at_s: asleep_at,
        active_at_10s: active10,
        step_ms_first_second: Stats::from_ms(&steps[..60]),
        step_ms: Stats::from_ms(&steps),
    }
}

// ---------------------------------------------------------------------------------------
// (c) Vehicle
// ---------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct VehicleReport {
    pub engine: &'static str,
    pub speed_at_5s: f32,
    pub distance_at_5s: f32,
    pub lateral_drift_at_5s: f32,
    pub heading_change_turn_deg: f32,
    pub turn_direction: &'static str,
    pub turn_radius_m: f32,
    pub min_up_dot: f32,
    pub mean_wheels_in_contact: f32,
    pub pass: bool,
    pub step_ms: Stats,
}

fn heading(s: &BodyState) -> f32 {
    let f = s.rotation * Vec3::Z;
    f.x.atan2(f.z)
}

pub fn vehicle<E: Engine>() -> VehicleReport {
    let mut e = E::new(WorldConfig {
        max_bodies: 64,
        threads: 1,
    });
    scene::ground(&mut e, 400.0);
    let car = e.add_vehicle(&scene::car(Vec3::new(0.0, 1.0, 0.0)));
    e.finish_setup();
    // Let the suspension settle for 1 s, then drive straight 5 s, then turn right 5 s.
    for _ in 0..60 {
        e.drive(car, 0.0, 0.0);
        e.step();
    }
    let s0 = e.vehicle_state(car).chassis;
    let mut steps = Vec::new();
    let mut min_up = 1.0f32;
    let mut contacts = 0u32;
    let (mut speed5, mut dist5, mut lat5) = (0.0, 0.0, 0.0);
    let mut yaw_rates = Vec::new();
    let mut speeds = Vec::new();
    let mut last_heading = heading(&s0);
    let mut turned = 0.0f32;
    for tick in 1..=600 {
        let (fwd, right) = if tick <= 300 { (1.0, 0.0) } else { (0.5, 1.0) };
        e.drive(car, fwd, right);
        let start = Instant::now();
        e.step();
        steps.push(ms_since(start));
        let vs = e.vehicle_state(car);
        let s = vs.chassis;
        contacts += vs.wheels_in_contact;
        min_up = min_up.min((s.rotation * Vec3::Y).y);
        let h = heading(&s);
        let mut dh = h - last_heading;
        if dh > std::f32::consts::PI {
            dh -= std::f32::consts::TAU;
        } else if dh < -std::f32::consts::PI {
            dh += std::f32::consts::TAU;
        }
        last_heading = h;
        if tick == 300 {
            let d = s.position - s0.position;
            speed5 = s.linear_velocity.length();
            dist5 = Vec3::new(d.x, 0.0, d.z).length();
            lat5 = d.x.abs();
        }
        if tick > 300 {
            turned += dh;
            if tick > 420 {
                yaw_rates.push(dh / DT);
                speeds.push(Vec3::new(s.linear_velocity.x, 0.0, s.linear_velocity.z).length());
            }
        }
    }
    let yaw = yaw_rates.iter().sum::<f32>() / yaw_rates.len() as f32;
    let speed = speeds.iter().sum::<f32>() / speeds.len() as f32;
    let radius = if yaw.abs() > 1e-4 {
        speed / yaw.abs()
    } else {
        f32::INFINITY
    };
    // Heading is atan2(x, z) with +Y up, so a right turn (towards -X when facing +Z) is negative.
    let direction = if turned < -0.1 {
        "right"
    } else if turned > 0.1 {
        "left"
    } else {
        "none"
    };
    let mean_contacts = contacts as f32 / 600.0;
    VehicleReport {
        engine: E::NAME,
        speed_at_5s: speed5,
        distance_at_5s: dist5,
        lateral_drift_at_5s: lat5,
        heading_change_turn_deg: turned.to_degrees(),
        turn_direction: direction,
        turn_radius_m: radius,
        min_up_dot: min_up,
        mean_wheels_in_contact: mean_contacts,
        pass: speed5 > 5.0
            && lat5 < 0.5
            && direction == "right"
            && min_up > 0.9
            && mean_contacts > 3.5,
        step_ms: Stats::from_ms(&steps),
    }
}

// ---------------------------------------------------------------------------------------
// (d) Scale
// ---------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ScaleReport {
    pub engine: &'static str,
    pub bodies: usize,
    pub threads: usize,
    pub steps: usize,
    pub setup_ms: f64,
    pub step_ms_all: Stats,
    pub step_ms_first_half: Stats,
    pub step_ms_second_half: Stats,
    pub active_at_end: usize,
    pub escaped_bodies: usize,
    /// OR of Jolt EPhysicsUpdateError bits over the run (0 = no capacity overflow).
    pub step_error_bits: u32,
    pub extract_all_ms: Stats,
    pub snapshot_bytes: usize,
    pub snapshot_save_ms: f64,
    pub snapshot_restore_ms: f64,
}

/// The scale scene: a 25 m radius, 12 m deep bowl with `count` bodies dropped into it in
/// 50 x 50 layers.
pub fn build_scale<E: Engine>(count: usize, threads: usize) -> (E, Vec<BodyId>) {
    let mut e = E::new(WorldConfig {
        max_bodies: count as u32 + 16,
        threads,
    });
    scene::bowl(&mut e, Vec3::ZERO, 25.0, 12.0);
    let bodies = scene::body_grid(count, Vec3::new(0.0, 13.0, 0.0), 50, 0.6, 7);
    let ids = e.add_dynamic(&bodies);
    e.finish_setup();
    (e, ids)
}

pub fn scale<E: Engine>(count: usize, threads: usize, steps: usize) -> ScaleReport {
    let start = Instant::now();
    let (mut e, ids) = build_scale::<E>(count, threads);
    let setup_ms = ms_since(start);
    let mut samples = Vec::with_capacity(steps);
    let mut extract = Vec::new();
    let mut state = Vec::with_capacity(ids.len());
    for tick in 0..steps {
        let t = Instant::now();
        e.step();
        samples.push(ms_since(t));
        if tick % 10 == 0 {
            let t = Instant::now();
            e.read(&ids, &mut state);
            extract.push(ms_since(t));
        }
    }
    e.read(&ids, &mut state);
    let escaped = state
        .iter()
        .filter(|s| s.position.y < -1.0 || !s.position.is_finite())
        .count();
    let t = Instant::now();
    let snap = e.save();
    let save_ms = ms_since(t);
    let t = Instant::now();
    e.restore(&snap);
    let restore_ms = ms_since(t);
    let half = steps / 2;
    ScaleReport {
        engine: E::NAME,
        bodies: count,
        threads,
        steps,
        setup_ms,
        step_ms_all: Stats::from_ms(&samples),
        step_ms_first_half: Stats::from_ms(&samples[..half]),
        step_ms_second_half: Stats::from_ms(&samples[half..]),
        active_at_end: e.active_count(),
        escaped_bodies: escaped,
        step_error_bits: e.step_errors(),
        extract_all_ms: Stats::from_ms(&extract),
        snapshot_bytes: snap.len(),
        snapshot_save_ms: save_ms,
        snapshot_restore_ms: restore_ms,
    }
}

// ---------------------------------------------------------------------------------------
// (e) Determinism
// ---------------------------------------------------------------------------------------

/// The determinism scene: 800 mixed bodies falling into a bowl, a pyramid, a driven car, a
/// walking character and a character riding a moving platform. Inputs are pure functions
/// of the tick number, so a replay after restore sees identical inputs.
pub struct DetScene<E: Engine> {
    pub e: E,
    pub tracked: Vec<BodyId>,
    platform: BodyId,
    car: u32,
    walker: u32,
    rider: u32,
}

fn det_platform(t: f32) -> Vec3 {
    Vec3::new(
        -15.0,
        1.0 + 0.4 * (t * 1.7).sin(),
        10.0 + 3.0 * (t * 1.1).sin(),
    )
}

impl<E: Engine> DetScene<E> {
    /// `perturb` moves one body's start position by one float step (negative control).
    pub fn build(threads: usize, perturb: bool) -> Self {
        let mut e = E::new(WorldConfig {
            max_bodies: 2048,
            threads,
        });
        scene::ground(&mut e, 100.0);
        scene::bowl(&mut e, Vec3::ZERO, 8.0, 4.0);
        scene::stairs(&mut e, -20.0, -10.0, 5, 0.3, 0.4);
        let mut grid = scene::body_grid(800, Vec3::new(0.0, 6.0, 0.0), 10, 0.6, 42);
        if perturb {
            grid[0].position.x = f32::from_bits(grid[0].position.x.to_bits() + 1);
        }
        let mut tracked = e.add_dynamic(&grid);
        tracked.extend(e.add_dynamic(&scene::pyramid(5, Vec3::new(20.0, 0.0, 0.0))));
        let platform = e.add_kinematic_box(det_platform(0.0), Vec3::new(1.5, 0.1, 1.5));
        let car = e.add_vehicle(&scene::car(Vec3::new(0.0, 1.0, 25.0)));
        tracked.push(e.vehicle_chassis(car));
        e.finish_setup();
        let ch = |p: Vec3| CharacterDesc {
            position: p,
            radius: 0.3,
            half_height: 0.6,
            max_slope_deg: 45.0,
            step_height: 0.35,
            stick_down: 0.5,
            padding: 0.02,
        };
        let walker = e.add_character(&ch(Vec3::new(-20.0, 0.0, -12.0)));
        let rider = e.add_character(&ch(det_platform(0.0) + Vec3::new(0.0, 0.15, 0.0)));
        Self {
            e,
            tracked,
            platform,
            car,
            walker,
            rider,
        }
    }

    /// One tick: inputs for `tick`, then the step.
    pub fn tick(&mut self, tick: u32) {
        let t = tick as f32 * DT;
        self.e.move_kinematic(self.platform, det_platform(t + DT));
        let walk = Vec3::new((t * 0.7).sin(), 0.0, (t * 0.7).cos()) * 2.0;
        self.e.update_character(self.walker, walk);
        self.e.update_character(self.rider, Vec3::ZERO);
        self.e.drive(self.car, 1.0, (t * 0.5).sin());
        self.e.step();
    }

    /// Hash of every tracked body's full state plus both characters, bit-exact.
    pub fn hash(&self, scratch: &mut Vec<BodyState>) -> u64 {
        let mut h = Fnv::default();
        self.e.read(&self.tracked, scratch);
        for s in scratch.iter() {
            h.vec3(s.position);
            h.quat(s.rotation);
            h.vec3(s.linear_velocity);
            h.vec3(s.angular_velocity);
            h.u32(u32::from(s.active));
        }
        for c in [self.walker, self.rider] {
            let s = self.e.character_state(c);
            h.vec3(s.position);
            h.vec3(s.velocity);
            h.vec3(s.ground_velocity);
            h.u32(u32::from(s.grounded));
        }
        h.0
    }
}

/// Per-tick hash chain of one run.
#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct RunHashes {
    pub ticks: u32,
    pub final_hash: String,
    pub chain_digest: String,
    pub hash_at: Vec<(u32, String)>,
}

pub fn det_run<E: Engine>(threads: usize, ticks: u32, perturb: bool) -> (RunHashes, Vec<u64>) {
    let mut s = DetScene::<E>::build(threads, perturb);
    let mut scratch = Vec::new();
    let mut chain = Fnv::default();
    let mut all = Vec::with_capacity(ticks as usize);
    for tick in 0..ticks {
        s.tick(tick);
        let h = s.hash(&mut scratch);
        chain.bytes(&h.to_le_bytes());
        all.push(h);
    }
    let pick = [99u32, 199, ticks - 1];
    (
        RunHashes {
            ticks,
            final_hash: format!("{:016x}", all[ticks as usize - 1]),
            chain_digest: format!("{:016x}", chain.0),
            hash_at: pick
                .iter()
                .filter(|t| **t < ticks)
                .map(|t| (*t + 1, format!("{:016x}", all[*t as usize])))
                .collect(),
        },
        all,
    )
}

#[derive(Serialize)]
pub struct SnapshotResult {
    pub snapshot_bytes: usize,
    pub save_ms: f64,
    pub restore_ms: f64,
    pub hash_200_uninterrupted: String,
    pub hash_200_first_pass: String,
    pub hash_200_after_restore: String,
    pub hash_200_restored_into_fresh_world: String,
    pub identical: bool,
}

/// Save at tick 100, run to 200, restore to 100, re-run to 200, compare. Also restores the
/// tick-100 snapshot into a separately built world (the late-join / server-to-client case).
pub fn snapshot_restore<E: Engine>(threads: usize, reference_200: u64) -> SnapshotResult {
    let mut s = DetScene::<E>::build(threads, false);
    let mut scratch = Vec::new();
    for tick in 0..100 {
        s.tick(tick);
    }
    let t = Instant::now();
    let snap = s.e.save();
    let save_ms = ms_since(t);
    for tick in 100..200 {
        s.tick(tick);
    }
    let first = s.hash(&mut scratch);
    let t = Instant::now();
    s.e.restore(&snap);
    let restore_ms = ms_since(t);
    for tick in 100..200 {
        s.tick(tick);
    }
    let second = s.hash(&mut scratch);

    let mut fresh = DetScene::<E>::build(threads, false);
    fresh.e.restore(&snap);
    for tick in 100..200 {
        fresh.tick(tick);
    }
    let third = fresh.hash(&mut scratch);
    SnapshotResult {
        snapshot_bytes: snap.len(),
        save_ms,
        restore_ms,
        hash_200_uninterrupted: format!("{reference_200:016x}"),
        hash_200_first_pass: format!("{first:016x}"),
        hash_200_after_restore: format!("{second:016x}"),
        hash_200_restored_into_fresh_world: format!("{third:016x}"),
        identical: first == second && second == third && first == reference_200,
    }
}

// ---------------------------------------------------------------------------------------
// Queries and FFI overhead
// ---------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct QueryReport {
    pub engine: &'static str,
    pub bodies: usize,
    pub single_position_read_ns: f64,
    pub bulk_read_ns_per_body: f64,
    pub raycast_ns: f64,
    pub raycast_hits: usize,
    /// Sanity check that both engines see the same pile: mean hit distance and normal.y.
    pub raycast_mean_distance: f32,
    pub raycast_mean_normal_y: f32,
    pub sphere_cast_ns: f64,
    pub sphere_cast_hits: usize,
    pub sphere_cast_mean_distance: f32,
}

pub fn queries<E: Engine>(bulk: impl Fn(&E, &[BodyId]) -> f64) -> QueryReport {
    let (mut e, ids) = build_scale::<E>(5000, 1);
    for _ in 0..300 {
        e.step();
    }
    // Single reads: one call per body.
    let rounds = 200;
    let t = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..rounds {
        for id in &ids {
            acc += e.body_position(*id).y;
        }
    }
    let single = ms_since(t) * 1e6 / (rounds * ids.len()) as f64;
    std::hint::black_box(acc);
    let bulk_ns = bulk(&e, &ids);

    let mut rng = Lcg::new(99);
    let origins: Vec<Vec3> = (0..10_000)
        .map(|_| Vec3::new(rng.range(-20.0, 20.0), 30.0, rng.range(-20.0, 20.0)))
        .collect();
    let t = Instant::now();
    let rays: Vec<Hit> = origins
        .iter()
        .filter_map(|o| e.cast_ray(*o, -Vec3::Y, 60.0))
        .collect();
    let ray_ns = ms_since(t) * 1e6 / origins.len() as f64;
    let t = Instant::now();
    let spheres: Vec<Hit> = origins
        .iter()
        .filter_map(|o| e.cast_sphere(0.2, *o, -Vec3::Y, 60.0))
        .collect();
    let sphere_ns = ms_since(t) * 1e6 / origins.len() as f64;
    let mean = |hits: &[Hit], f: fn(&Hit) -> f32| {
        hits.iter().map(f).sum::<f32>() / hits.len().max(1) as f32
    };
    QueryReport {
        engine: E::NAME,
        bodies: ids.len(),
        single_position_read_ns: single,
        bulk_read_ns_per_body: bulk_ns,
        raycast_ns: ray_ns,
        raycast_hits: rays.len(),
        raycast_mean_distance: mean(&rays, |h| h.distance),
        raycast_mean_normal_y: mean(&rays, |h| h.normal.y),
        sphere_cast_ns: sphere_ns,
        sphere_cast_hits: spheres.len(),
        sphere_cast_mean_distance: mean(&spheres, |h| h.distance),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jolt::JoltEngine;
    use crate::rapier::RapierEngine;

    /// Two runs of the determinism scene hash identically, and a one-ulp change to one
    /// body's start position changes the hash (so the hash really covers the state).
    fn replay_is_bit_exact<E: Engine>() {
        let (a, _) = det_run::<E>(1, 60, false);
        let (b, _) = det_run::<E>(1, 60, false);
        assert_eq!(a, b);
        let (p, _) = det_run::<E>(1, 60, true);
        assert_ne!(a.final_hash, p.final_hash);
    }

    #[test]
    fn jolt_replay_is_bit_exact() {
        replay_is_bit_exact::<JoltEngine>();
    }

    #[test]
    fn rapier_replay_is_bit_exact() {
        replay_is_bit_exact::<RapierEngine>();
    }
}
