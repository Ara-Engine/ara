//! Glint's course as data: a turtle walks the route and emits the pieces everything else is
//! built from (the visual kit, the colliders and their `Surface` faces, the waypoints the camera
//! follows, the pickups, checkpoints, movers, bars, the cannon and the goal). One table, so the
//! look, the physics and the scripts cannot disagree about where a piece is.
//! Frame: Y up, right-handed, metres; heading 0 is +Z and the heading turns toward +X as it grows.
//! Orientation of a piece is `Ry(heading) * Rx(pitch) * Rz(roll)`; Ara's Euler angles are
//! `Rx * Ry * Rz`, so the scene stores `to_euler(XYZ)` of that quaternion.
use glam::{EulerRot, Quat, Vec3};

/// Gravity the cannon's ballistic solution assumes (m/s squared).
pub const GRAVITY: f32 = 9.81;
/// The route starts angled toward the goal, so the goal pillar is in view from the start.
pub const START_HEADING: f32 = -50.0;
/// The banked turn, degrees (to the right).
pub const TURN: f32 = 40.0;
/// Slab thickness (m).
pub const THICK: f32 = 0.8;

/// How a piece is finished and decorated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A wide deck with edge strips.
    Deck,
    /// A pad that carries a checkpoint, cannon or the goal: a trimmed square.
    Pad,
    /// A narrow beam.
    Beam,
    /// One segment of a banked curve.
    Bank,
}

/// A static, walkable box.
#[derive(Clone, Debug)]
pub struct Piece {
    pub id: String,
    pub name: &'static str,
    pub kind: Kind,
    /// Centre of the box.
    pub center: Vec3,
    /// Half extents: X across, Y thickness, Z along.
    pub half: Vec3,
    pub rot: Quat,
}

impl Piece {
    /// The Euler angles the scene stores, degrees.
    pub fn euler(&self) -> [f32; 3] {
        let (x, y, z) = self.rot.to_euler(EulerRot::XYZ);
        [x.to_degrees(), y.to_degrees(), z.to_degrees()]
    }
}

/// A platform sliding on a sine along `travel`.
#[derive(Clone, Debug)]
pub struct Slider {
    pub id: String,
    pub origin: Vec3,
    pub travel: Vec3,
    pub half: Vec3,
    pub rot: Quat,
    pub period: u32,
    pub phase: u32,
}

/// A spinning bar over a deck.
#[derive(Clone, Debug)]
pub struct Bar {
    pub id: String,
    pub pivot: Vec3,
    pub yaw: f32,
    pub rate: f32,
}

/// The cannon and its ballistic solution.
#[derive(Clone, Debug)]
pub struct Cannon {
    /// Where the ball is held while it loads.
    pub breech: Vec3,
    /// Where it leaves the barrel.
    pub muzzle: Vec3,
    /// Launch velocity (m/s).
    pub launch: Vec3,
    /// Barrel heading and pitch, degrees.
    pub heading: f32,
    pub pitch: f32,
}

/// The whole route.
#[derive(Clone, Debug, Default)]
pub struct Course {
    pub pieces: Vec<Piece>,
    pub sliders: Vec<Slider>,
    pub bars: Vec<Bar>,
    /// Checkpoint pad tops: position of the spawn point above each.
    pub checkpoints: Vec<(Vec3, f32)>,
    pub pickups: Vec<Vec3>,
    /// The centre line the camera follows, from the start to the goal.
    pub waypoints: Vec<Vec3>,
    pub cannon: Option<Cannon>,
    /// Goal pad centre on the surface.
    pub goal: Vec3,
    /// Where the ball starts.
    pub start: Vec3,
}

struct Turtle {
    /// The start of the next piece: the middle of its leading edge, on the surface.
    pos: Vec3,
    heading: f32,
    course: Course,
    count: u32,
}

fn heading_dir(heading: f32) -> Vec3 {
    let h = heading.to_radians();
    Vec3::new(h.sin(), 0.0, h.cos())
}

fn orientation(heading: f32, pitch: f32, roll: f32) -> Quat {
    Quat::from_rotation_y(heading.to_radians())
        * Quat::from_rotation_x((-pitch).to_radians())
        * Quat::from_rotation_z(roll.to_radians())
}

impl Turtle {
    fn id(&mut self, prefix: &str) -> String {
        self.count += 1;
        format!("e_{prefix}{}", self.count)
    }

    /// A piece `length` long (along the heading), `width` across, rising at `pitch` degrees
    /// and rolled by `roll`; advances the turtle to its far edge.
    fn piece(
        &mut self,
        name: &'static str,
        kind: Kind,
        length: f32,
        width: f32,
        pitch: f32,
        roll: f32,
    ) -> Vec3 {
        let rot = orientation(self.heading, pitch, roll);
        let forward = rot * Vec3::Z;
        let up = rot * Vec3::Y;
        let center = self.pos + forward * (length * 0.5) - up * (THICK * 0.5);
        let id = self.id("p");
        self.course.pieces.push(Piece {
            id,
            name,
            kind,
            center,
            half: Vec3::new(width * 0.5, THICK * 0.5, length * 0.5),
            rot,
        });
        self.course.waypoints.push(self.pos + forward * 0.0);
        let end = self.pos + forward * length;
        self.pos = end;
        center + up * (THICK * 0.5)
    }

    fn gap(&mut self, length: f32) {
        self.course.waypoints.push(self.pos);
        self.pos += heading_dir(self.heading) * length;
    }
}

/// Walks the route and returns the course.
pub fn build() -> Course {
    let mut t = Turtle {
        pos: Vec3::new(0.0, 0.0, -9.0),
        heading: START_HEADING,
        course: Course::default(),
        count: 0,
    };
    // 1. Launch deck: wide, the title view's foreground.
    t.piece("Launch deck", Kind::Pad, 18.0, 12.0, 0.0, 0.0);
    let d0 = heading_dir(t.heading);
    t.course.start = Vec3::new(0.0, 0.6, -9.0) + d0 * 5.5;
    // 2. The wide straight: it teaches steering. Orbs run down its middle.
    let a = t.pos;
    t.piece("Wide straight", Kind::Deck, 46.0, 7.0, 0.0, 0.0);
    for i in 0..8 {
        t.course
            .pickups
            .push(a + d0 * (5.0 + i as f32 * 5.0) + Vec3::new(0.0, 0.9, 0.0));
    }
    // 3. The first gap, and checkpoint 1 on the pad beyond.
    t.gap(6.0);
    let pad = t.pos;
    t.piece("Checkpoint pad 1", Kind::Pad, 12.0, 9.0, 0.0, 0.0);
    t.course.checkpoints.push((pad + d0 * 6.0, t.heading));
    // 4. A climbing ramp: orbs follow the rise.
    let r = t.pos;
    t.piece("Climb", Kind::Deck, 28.0, 5.0, 6.0, 0.0);
    for i in 0..6 {
        let s = 3.0 + i as f32 * 4.4;
        t.course
            .pickups
            .push(r + d0 * s + Vec3::new(0.0, 0.9 + s * 6f32.to_radians().tan(), 0.0));
    }
    // 5. The beam: narrow and long, with a pair of orbs to coax a line.
    let b = t.pos;
    t.piece("Beam", Kind::Beam, 34.0, 1.7, 0.0, 0.0);
    for i in 0..5 {
        t.course
            .pickups
            .push(b + d0 * (6.0 + i as f32 * 5.5) + Vec3::new(0.0, 0.9, 0.0));
    }
    // 6. A banked turn to the right over 12 segments.
    let (segments, radius, width, bank) = (12usize, 24.0f32, 7.0f32, 16.0f32);
    let arc = radius * TURN.to_radians();
    let seg = arc / segments as f32;
    for i in 0..segments {
        let mid = t.heading - TURN / segments as f32 * 0.5;
        let rot = orientation(mid, 0.0, bank);
        let forward = rot * Vec3::Z;
        let up = rot * Vec3::Y;
        // Extend each segment a little so neighbours overlap along the arc.
        let length = seg * 1.12;
        let center = t.pos + heading_dir(mid) * (seg * 0.5) - up * (THICK * 0.5);
        let id = t.id("b");
        t.course.pieces.push(Piece {
            id,
            name: "Banked turn",
            kind: Kind::Bank,
            center,
            half: Vec3::new(width * 0.5, THICK * 0.5, length * 0.5),
            rot,
        });
        let _ = forward;
        t.course.waypoints.push(t.pos);
        if i % 3 == 1 {
            t.course
                .pickups
                .push(t.pos + heading_dir(mid) * (seg * 0.5) + Vec3::new(0.0, 0.9, 0.0));
        }
        t.pos += heading_dir(mid) * seg;
        t.heading = mid - TURN / segments as f32 * 0.5;
    }
    // 7. Checkpoint 2.
    let pad = t.pos;
    let dir = heading_dir(t.heading);
    t.piece("Checkpoint pad 2", Kind::Pad, 10.0, 9.0, 0.0, 0.0);
    t.course.checkpoints.push((pad + dir * 5.0, t.heading));
    // 8. Two sliders across a gap: forward-and-back along the route, out of phase.
    let gap_start = t.pos;
    let gap = 22.0;
    let top = t.pos.y;
    for (n, (s, phase)) in [(6.5f32, 0u32), (15.5, 135)].into_iter().enumerate() {
        let origin = gap_start + dir * s - Vec3::new(0.0, THICK * 0.5, 0.0);
        t.course.sliders.push(Slider {
            id: format!("e_m{}", n + 1),
            origin,
            travel: dir * 3.4,
            half: Vec3::new(2.2, THICK * 0.5, 2.2),
            rot: orientation(t.heading, 0.0, 0.0),
            period: 270,
            phase,
        });
    }
    t.gap(gap);
    let _ = top;
    // 9. Landing pad, then the spinner deck.
    t.piece("Landing pad", Kind::Pad, 9.0, 9.0, 0.0, 0.0);
    let sd = t.pos;
    t.piece("Spinner deck", Kind::Deck, 30.0, 8.0, 0.0, 0.0);
    for (i, (s, yaw, rate)) in [
        (8.0f32, 0.0f32, 80.0f32),
        (16.0, 60.0, -95.0),
        (24.0, 20.0, 110.0),
    ]
    .into_iter()
    .enumerate()
    {
        t.course.bars.push(Bar {
            id: format!("e_b{}", i + 1),
            pivot: sd + dir * s + Vec3::new(0.0, 0.5, 0.0),
            yaw,
            rate,
        });
    }
    for i in 0..4 {
        t.course
            .pickups
            .push(sd + dir * (4.0 + i as f32 * 7.0) + Vec3::new(0.0, 1.9, 0.0));
    }
    // 10. Checkpoint 3 and the cannon pad.
    let cp = t.pos;
    t.piece("Cannon pad", Kind::Pad, 20.0, 12.0, 0.0, 0.0);
    t.course.checkpoints.push((cp + dir * 4.0, t.heading));
    // The cannon sits at the far end of the pad, its barrel along the heading.
    let breech = t.pos - dir * 6.0 + Vec3::new(0.0, 0.5, 0.0);
    let muzzle = breech + dir * 3.2 + Vec3::new(0.0, 1.6, 0.0);
    // 11. The flight: a 72 m jump onto the goal pad, 8 m lower.
    let distance = 76.0;
    let goal_top = t.pos.y - 8.0;
    let goal_len = 22.0;
    let goal_start = t.pos + dir * distance + Vec3::new(0.0, goal_top - t.pos.y, 0.0);
    t.course.goal = goal_start + dir * (goal_len * 0.45) + Vec3::new(0.0, 0.0, 0.0);
    let target = t.course.goal + Vec3::new(0.0, 0.5, 0.0);
    let flat = Vec3::new(target.x - muzzle.x, 0.0, target.z - muzzle.z);
    let d = flat.length();
    let dy = target.y - muzzle.y;
    let theta = 36f32.to_radians();
    let v = (GRAVITY * d * d / (2.0 * theta.cos().powi(2) * (d * theta.tan() - dy))).sqrt();
    let launch = flat.normalize() * (v * theta.cos()) + Vec3::Y * (v * theta.sin());
    t.course.cannon = Some(Cannon {
        breech,
        muzzle,
        launch,
        heading: t.heading,
        pitch: 36.0,
    });
    t.course.waypoints.push(t.pos);
    t.pos = goal_start;
    t.piece("Goal pad", Kind::Pad, goal_len, 18.0, 0.0, 0.0);
    t.course.waypoints.push(t.course.goal);
    t.course
}
