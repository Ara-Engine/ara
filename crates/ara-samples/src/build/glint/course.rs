//! Glint's course as data: a turtle walks the route and emits the pieces everything else is
//! built from (the visual kit, the colliders and their `Surface` faces, the waypoints the camera
//! follows, the pickups, checkpoints, movers, bars, the cannon, the goal, the decorative rings
//! the route passes through and the autopilot's plan). One table, so the look, the physics, the
//! scripts and the replay cannot disagree about where a piece is.
//! Frame: Y up, right-handed, metres; heading 0 is +Z and the heading turns toward +X as it grows.
//! Orientation of a piece is `Ry(heading) * Rx(-pitch) * Rz(roll)`; Ara's Euler angles are
//! `Rx * Ry * Rz`, so the scene stores `to_euler(XYZ)` of that quaternion.
use glam::{EulerRot, Quat, Vec3};

/// Gravity the ballistic solutions assume (m/s squared).
pub const GRAVITY: f32 = 9.81;
/// The ball's top rolling speed (m/s); `BallController.TopSpeed` is set from it.
pub const TOP_SPEED: f32 = 9.5;
/// The route starts angled toward the goal, so the goal is in view from the start.
pub const START_HEADING: f32 = -50.0;
/// The banked turn, degrees (to the right).
pub const TURN: f32 = 60.0;
/// Slab thickness (m).
pub const THICK: f32 = 0.8;

/// How a piece is finished and decorated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A deck or a ramp with edge trim.
    Deck,
    /// A pad that carries a checkpoint, the cannon or the goal: polished, on a pylon.
    Pad,
    /// A narrow beam.
    Beam,
    /// One segment of a banked curve.
    Bank,
    /// A glass bridge: a transmission-glass slab in a metal frame, the drop visible through it.
    Glass,
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
        euler(self.rot)
    }
}

/// Scene Euler angles (degrees) of a rotation.
pub fn euler(rot: Quat) -> [f32; 3] {
    let (x, y, z) = rot.to_euler(EulerRot::XYZ);
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

/// A solid glass rail.
#[derive(Clone, Debug)]
pub struct Wall {
    pub id: String,
    pub center: Vec3,
    pub half: Vec3,
    pub rot: Quat,
}

/// A decorative ring the route passes through (no collision).
#[derive(Clone, Debug)]
pub struct Ring {
    pub at: Vec3,
    /// The ring's axis is this rotation's +Z.
    pub rot: Quat,
    pub radius: f32,
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
    /// Barrel heading, degrees.
    pub heading: f32,
    /// Seconds from the muzzle to the goal pad.
    pub flight: f32,
}

impl Cannon {
    /// The ball's centre `t` seconds after the shot.
    pub fn arc(&self, t: f32) -> Vec3 {
        self.muzzle + self.launch * t - Vec3::Y * (0.5 * GRAVITY * t * t)
    }
}

/// One step of the autopilot's plan (`replay/route.json`): how a perfect, plain run goes.
#[derive(Clone, Debug)]
pub enum Step {
    /// Roll along the line to `at` with the stick at `speed`.
    Roll { at: Vec3, speed: f32 },
    /// Roll to `at` and jump there.
    Jump { at: Vec3 },
    /// Wait until the slider's centre is within `reach`, then roll onto it and ride it.
    Board { slider: String, reach: f32 },
    /// Wait (riding) until `at` is within `reach`, then roll to it.
    Leave { at: Vec3, reach: f32 },
    /// Roll into the cannon's intake and wait for the landing.
    Cannon { at: Vec3 },
}

/// The whole route.
#[derive(Clone, Debug, Default)]
pub struct Course {
    pub pieces: Vec<Piece>,
    pub walls: Vec<Wall>,
    pub rings: Vec<Ring>,
    pub sliders: Vec<Slider>,
    pub bars: Vec<Bar>,
    /// Checkpoints: a point on the pad's surface and the route's heading there.
    pub checkpoints: Vec<(Vec3, f32)>,
    pub pickups: Vec<Vec3>,
    /// The centre line the camera follows, from the start to the goal.
    pub waypoints: Vec<Vec3>,
    pub plan: Vec<Step>,
    pub cannon: Option<Cannon>,
    /// Goal pad centre on the surface.
    pub goal: Vec3,
    /// Where the ball starts.
    pub start: Vec3,
}

/// A laid piece's centre line on its surface.
#[derive(Clone, Copy)]
struct Span {
    start: Vec3,
    forward: Vec3,
}

impl Span {
    /// The point `s` metres along the piece.
    fn at(&self, s: f32) -> Vec3 {
        self.start + self.forward * s
    }
    /// Where a ball's centre passes `s` metres along (an orb's place).
    fn orb(&self, s: f32) -> Vec3 {
        self.at(s) + Vec3::Y * 0.9
    }
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

    fn dir(&self) -> Vec3 {
        heading_dir(self.heading)
    }

    /// A piece `length` long (along the heading), `width` across, rising at `pitch` degrees;
    /// advances the turtle to its far edge.
    fn piece(
        &mut self,
        name: &'static str,
        kind: Kind,
        length: f32,
        width: f32,
        pitch: f32,
    ) -> Span {
        let start = self.pos;
        let span = self.side(name, kind, start, self.heading, length, width, pitch, 0.0);
        self.course.waypoints.push(start);
        self.pos = span.at(length);
        span
    }

    /// A piece laid from `start` along `heading` without moving the turtle or the camera line;
    /// `grow` lengthens both ends (so pieces that meet at an angle leave no wedge open).
    #[allow(clippy::too_many_arguments)]
    fn side(
        &mut self,
        name: &'static str,
        kind: Kind,
        start: Vec3,
        heading: f32,
        length: f32,
        width: f32,
        pitch: f32,
        grow: f32,
    ) -> Span {
        let rot = orientation(heading, pitch, 0.0);
        let (forward, up) = (rot * Vec3::Z, rot * Vec3::Y);
        let id = self.id("p");
        self.course.pieces.push(Piece {
            id,
            name,
            kind,
            center: start + forward * (length * 0.5) - up * (THICK * 0.5),
            half: Vec3::new(width * 0.5, THICK * 0.5, length * 0.5 + grow),
            rot,
        });
        Span { start, forward }
    }

    /// A glass rail on a piece: `at` and `half` in the piece's frame (from its centre).
    fn wall(&mut self, piece: usize, at: Vec3, half: Vec3) {
        let p = &self.course.pieces[piece];
        let (center, rot) = (p.center + p.rot * at, p.rot);
        let id = self.id("w");
        self.course.walls.push(Wall {
            id,
            center,
            half,
            rot,
        });
    }

    /// Open air: `length` ahead and `rise` up (negative: down) to the next piece.
    fn gap(&mut self, length: f32, rise: f32) {
        self.course.waypoints.push(self.pos);
        self.pos += self.dir() * length + Vec3::Y * rise;
    }

    fn roll(&mut self, at: Vec3, speed: f32) {
        self.course.plan.push(Step::Roll { at, speed });
    }

    /// A ring around the route at `at`, facing along the heading.
    fn ring(&mut self, at: Vec3, radius: f32) {
        let rot = orientation(self.heading, 0.0, 0.0);
        self.course.rings.push(Ring { at, rot, radius });
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
    // 1. Launch deck: wide, glass at the back and sides; the title view's foreground.
    let s = t.piece("Launch deck", Kind::Pad, 18.0, 12.0, 0.0);
    t.course.start = s.at(5.5) + Vec3::Y * 0.6;
    let h = t.course.pieces[0].half;
    t.wall(
        0,
        Vec3::new(0.0, h.y + 0.5, -h.z + 0.1),
        Vec3::new(h.x, 0.5, 0.05),
    );
    for side in [-1.0f32, 1.0] {
        t.wall(
            0,
            Vec3::new(side * (h.x - 0.1), h.y + 0.5, 0.0),
            Vec3::new(0.05, 0.5, h.z),
        );
    }
    // 2. The wide straight teaches steering: orbs down its middle, two rings to roll through.
    let s = t.piece("Wide straight", Kind::Deck, 40.0, 8.0, 0.0);
    for i in 0..8 {
        t.course.pickups.push(s.orb(5.0 + i as f32 * 4.5));
    }
    t.ring(s.at(12.0) + Vec3::Y * 2.0, 6.5);
    t.ring(s.at(30.0) + Vec3::Y * 2.0, 6.5);
    // 3. The first gap asks for a jump; checkpoint 1 is on the pad beyond.
    t.course.plan.push(Step::Jump { at: s.at(39.2) });
    t.gap(6.0, 0.0);
    let s = t.piece("Checkpoint pad 1", Kind::Pad, 12.0, 10.0, 0.0);
    t.course.checkpoints.push((s.at(6.0), t.heading));
    t.roll(s.at(11.5), 1.0);
    // 4. A descent, a run-up and a kicker: full speed carries the ball over the gap onto a
    //    glass bridge 2.5 m lower, through three orbs on the arc.
    let s = t.piece("Descent", Kind::Deck, 30.0, 6.0, -8.0);
    for i in 0..5 {
        t.course.pickups.push(s.orb(4.0 + i as f32 * 5.5));
    }
    t.piece("Run-up", Kind::Deck, 8.0, 6.0, 0.0);
    let s = t.piece("Kicker", Kind::Deck, 7.0, 6.0, 12.0);
    let lip = s.at(7.0);
    let leave = s.forward * TOP_SPEED;
    for time in [0.2f32, 0.45, 0.7] {
        t.course
            .pickups
            .push(lip + Vec3::Y * 0.5 + leave * time - Vec3::Y * (0.5 * GRAVITY * time * time));
    }
    t.roll(lip, 1.0);
    t.gap(5.0, -(lip.y - s.at(0.0).y) - 2.5);
    let s = t.piece("Landing deck", Kind::Glass, 18.0, 9.0, 0.0);
    t.ring(s.at(-2.0) + Vec3::Y * 2.6, 5.5);
    t.roll(s.at(17.0), 1.0);
    // 5. The climb back up, narrower.
    let s = t.piece("Climb", Kind::Deck, 28.0, 5.0, 8.0);
    for i in 0..5 {
        t.course.pickups.push(s.orb(3.0 + i as f32 * 5.5));
    }
    t.roll(s.at(28.0), 1.0);
    // 6. The split: the beam straight ahead is short, narrow and paved with orbs; the detour
    //    to the left is wide and longer.
    let s = t.piece("Split pad", Kind::Pad, 8.0, 10.0, 0.0);
    t.roll(s.at(8.0), 1.0);
    let (fork, heading) = (t.pos, t.heading);
    let left = orientation(heading, 0.0, 0.0) * Vec3::X;
    let mut at = fork + left * 3.0;
    for (name, turn, length) in [
        ("Detour out", 30.0f32, 12.0f32),
        ("Detour", 0.0, 11.2),
        ("Detour back", -30.0, 12.0),
    ] {
        t.side(name, Kind::Deck, at, heading + turn, length, 4.0, 0.0, 0.9);
        at += heading_dir(heading + turn) * length;
    }
    let s = t.piece("Beam", Kind::Beam, 32.0, 1.6, 0.0);
    for i in 0..6 {
        t.course.pickups.push(s.orb(4.0 + i as f32 * 4.8));
    }
    t.ring(s.at(8.0) + Vec3::Y * 1.4, 4.2);
    t.ring(s.at(24.0) + Vec3::Y * 1.4, 4.2);
    t.roll(s.at(32.0), 0.85);
    let s = t.piece("Rejoin pad", Kind::Pad, 8.0, 10.0, 0.0);
    t.roll(s.at(8.0), 1.0);
    // 7. A banked turn to the right, descending, with a glass rail on the outside.
    let (segments, radius, width, bank, pitch) = (14usize, 26.0f32, 7.0f32, 14.0f32, -3.0f32);
    let seg = radius * TURN.to_radians() / segments as f32;
    for i in 0..segments {
        let mid = t.heading - TURN / segments as f32 * 0.5;
        let rot = orientation(mid, pitch, bank);
        let (forward, up) = (rot * Vec3::Z, rot * Vec3::Y);
        let id = t.id("b");
        // Each segment is a little long, so neighbours overlap along the arc.
        let half = Vec3::new(width * 0.5, THICK * 0.5, seg * 0.56);
        t.course.pieces.push(Piece {
            id,
            name: "Banked turn",
            kind: Kind::Bank,
            center: t.pos + forward * (seg * 0.5) - up * (THICK * 0.5),
            half,
            rot,
        });
        let index = t.course.pieces.len() - 1;
        t.wall(
            index,
            Vec3::new(half.x - 0.1, half.y + 0.45, 0.0),
            Vec3::new(0.05, 0.45, half.z),
        );
        t.course.waypoints.push(t.pos);
        if i % 3 == 1 {
            t.course
                .pickups
                .push(t.pos + forward * (seg * 0.5) + up * 0.9);
        }
        t.pos += forward * seg;
        t.heading = mid - TURN / segments as f32 * 0.5;
        if i % 2 == 1 {
            let at = t.pos;
            t.roll(at, 1.0);
        }
    }
    // 8. Checkpoint 2, then two sliders across a gap. They move along the route half a period
    //    apart: each meets the pad behind it, then the platform ahead.
    let s = t.piece("Checkpoint pad 2", Kind::Pad, 12.0, 10.0, 0.0);
    t.course.checkpoints.push((s.at(5.0), t.heading));
    t.roll(s.at(10.8), 0.6);
    let (reach, half, clear) = (3.2f32, 2.2f32, 0.2f32);
    let gap = 4.0 * reach + 4.0 * half + 2.0 * clear + 0.3;
    let dir = t.dir();
    for (n, (along, phase)) in [
        (reach + half + clear, 0u32),
        (gap - reach - half - clear, 135),
    ]
    .into_iter()
    .enumerate()
    {
        t.course.sliders.push(Slider {
            id: format!("e_m{}", n + 1),
            origin: t.pos + dir * along - Vec3::Y * (THICK * 0.5),
            travel: dir * reach,
            half: Vec3::new(half, THICK * 0.5, half),
            rot: orientation(t.heading, 0.0, 0.0),
            period: 270,
            phase,
        });
    }
    t.course.plan.push(Step::Board {
        slider: "e_m1".into(),
        reach: 1.2 + clear + half + 0.6,
    });
    t.course.plan.push(Step::Board {
        slider: "e_m2".into(),
        reach: 2.0 * half + 0.3 + 0.6,
    });
    t.gap(gap, 0.0);
    // 9. Landing pad, then the spinner deck: three bars to jump, an orb over each.
    let s = t.piece("Landing pad", Kind::Pad, 9.0, 9.0, 0.0);
    t.course.plan.push(Step::Leave {
        at: s.at(1.5),
        reach: half + clear + 1.5 + 0.6,
    });
    t.roll(s.at(8.5), 1.0);
    let s = t.piece("Spinner deck", Kind::Deck, 44.0, 8.0, 0.0);
    for (i, (along, yaw, rate)) in [
        (8.0f32, 0.0f32, 80.0f32),
        (22.0, 60.0, -95.0),
        (36.0, 20.0, 110.0),
    ]
    .into_iter()
    .enumerate()
    {
        t.course.bars.push(Bar {
            id: format!("e_b{}", i + 1),
            pivot: s.at(along) + Vec3::Y * 0.5,
            yaw,
            rate,
        });
        t.course.pickups.push(s.at(along) + Vec3::Y * 2.1);
        t.course.plan.push(Step::Jump {
            at: s.at(along - 5.5),
        });
    }
    t.roll(s.at(44.0), 1.0);
    // 10. Checkpoint 3 and the ramp up the cannon tower.
    let s = t.piece("Checkpoint pad 3", Kind::Pad, 10.0, 10.0, 0.0);
    t.course.checkpoints.push((s.at(5.0), t.heading));
    t.roll(s.at(10.0), 1.0);
    let s = t.piece("Tower ramp", Kind::Deck, 20.0, 5.0, 10.0);
    for i in 0..3 {
        t.course.pickups.push(s.orb(4.0 + i as f32 * 6.0));
    }
    t.roll(s.at(20.0), 1.0);
    let s = t.piece("Cannon deck", Kind::Pad, 16.0, 14.0, 0.0);
    // 11. The cannon and the shot: 80 m across and 10 m down onto the goal plaza, through two
    //     rings and five orbs on the arc.
    let theta = 36f32.to_radians();
    let breech = s.at(9.0) + Vec3::Y * 0.5;
    let muzzle = breech + dir * (3.6 * theta.cos()) + Vec3::Y * (3.6 * theta.sin());
    t.course.plan.push(Step::Cannon { at: breech });
    t.gap(80.0, -10.0);
    let s = t.piece("Goal plaza", Kind::Pad, 26.0, 26.0, 0.0);
    t.course.goal = s.at(13.0);
    let target = t.course.goal + Vec3::Y * 0.5;
    let flat = Vec3::new(target.x - muzzle.x, 0.0, target.z - muzzle.z);
    let (d, dy) = (flat.length(), target.y - muzzle.y);
    let v = (GRAVITY * d * d / (2.0 * theta.cos().powi(2) * (d * theta.tan() - dy))).sqrt();
    let cannon = Cannon {
        breech,
        muzzle,
        launch: flat.normalize() * (v * theta.cos()) + Vec3::Y * (v * theta.sin()),
        heading: t.heading,
        flight: d / (v * theta.cos()),
    };
    for f in [0.2f32, 0.35, 0.5, 0.65, 0.8] {
        t.course.pickups.push(cannon.arc(cannon.flight * f));
    }
    for f in [0.3f32, 0.72] {
        let time = cannon.flight * f;
        let velocity = cannon.launch - Vec3::Y * (GRAVITY * time);
        t.course.rings.push(Ring {
            at: cannon.arc(time),
            rot: Quat::from_rotation_arc(Vec3::Z, velocity.normalize()),
            radius: 5.0,
        });
    }
    t.course.cannon = Some(cannon);
    t.course.waypoints.push(t.course.goal);
    t.course
}
