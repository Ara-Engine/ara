//! ORCA half-planes and the 2D/3D linear programs that pick a collision-free velocity.
//!
//! Follows "Reciprocal n-Body Collision Avoidance" (van den Berg, Guy, Lin and
//! Manocha, 2011). Written for Ara: fixed-size scratch, no allocation, f32 maths
//! in a fixed order so results are identical on any worker.

/// Lines a single agent can hold: wall half-planes first, then neighbours.
pub(crate) const MAX_LINES: usize = 40;
const EPSILON: f32 = 1e-5;

/// 2D vector in the XZ plane (`x`, `z`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct V2 {
    pub x: f32,
    pub z: f32,
}
impl V2 {
    pub(crate) const fn new(x: f32, z: f32) -> Self {
        Self { x, z }
    }
    pub(crate) fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.z * o.z
    }
    pub(crate) fn len_sq(self) -> f32 {
        self.dot(self)
    }
    pub(crate) fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.z * s)
    }
    pub(crate) fn normalized(self) -> Self {
        let len = self.len_sq().sqrt();
        if len > 0.0 {
            self.scale(1.0 / len)
        } else {
            self
        }
    }
}
impl std::ops::Add for V2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.z + o.z)
    }
}
impl std::ops::Sub for V2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.z - o.z)
    }
}
impl std::ops::Neg for V2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.z)
    }
}
/// 2D cross product; positive when `b` is counter-clockwise (left) of `a`.
pub(crate) fn det(a: V2, b: V2) -> f32 {
    a.x * b.z - a.z * b.x
}

/// Permitted velocities lie to the left of `direction` through `point`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Line {
    pub point: V2,
    pub direction: V2,
}

/// Fixed-capacity line list; extra constraints beyond capacity are dropped
/// (callers add the nearest first, so the most relevant ones survive).
pub(crate) struct Lines {
    pub lines: [Line; MAX_LINES],
    pub len: usize,
}
impl Lines {
    pub(crate) fn new() -> Self {
        Self {
            lines: [Line::default(); MAX_LINES],
            len: 0,
        }
    }
    pub(crate) fn push(&mut self, line: Line) {
        if self.len < MAX_LINES {
            self.lines[self.len] = line;
            self.len += 1;
        }
    }
    pub(crate) fn as_slice(&self) -> &[Line] {
        &self.lines[..self.len]
    }
}

/// Reciprocal constraint against a moving disc. `rel_pos` = other - self,
/// `rel_vel` = self - other velocity. `responsibility` is 0.5 between agents
/// (each avoids half) and 1 against fixed obstacles.
pub(crate) fn agent_line(
    velocity: V2,
    rel_pos: V2,
    rel_vel: V2,
    combined_radius: f32,
    inv_horizon: f32,
    inv_dt: f32,
    responsibility: f32,
) -> Line {
    let dist_sq = rel_pos.len_sq();
    let r_sq = combined_radius * combined_radius;
    let (direction, u);
    if dist_sq > r_sq {
        // Vector from the truncated cone's cut-off centre to the relative velocity.
        let w = rel_vel - rel_pos.scale(inv_horizon);
        let w_len_sq = w.len_sq();
        let dot1 = w.dot(rel_pos);
        if dot1 < 0.0 && dot1 * dot1 > r_sq * w_len_sq {
            // Project onto the cut-off circle.
            let w_len = w_len_sq.sqrt();
            let unit = w.scale(1.0 / w_len);
            direction = V2::new(unit.z, -unit.x);
            u = unit.scale(combined_radius * inv_horizon - w_len);
        } else {
            // Project onto the nearer leg of the cone.
            let leg = (dist_sq - r_sq).sqrt();
            direction = if det(rel_pos, w) > 0.0 {
                V2::new(
                    rel_pos.x * leg - rel_pos.z * combined_radius,
                    rel_pos.x * combined_radius + rel_pos.z * leg,
                )
                .scale(1.0 / dist_sq)
            } else {
                -V2::new(
                    rel_pos.x * leg + rel_pos.z * combined_radius,
                    -rel_pos.x * combined_radius + rel_pos.z * leg,
                )
                .scale(1.0 / dist_sq)
            };
            u = direction.scale(rel_vel.dot(direction)) - rel_vel;
        }
    } else {
        // Already overlapping: resolve within one step.
        let w = rel_vel - rel_pos.scale(inv_dt);
        let w_len = w.len_sq().sqrt();
        let unit = if w_len > 0.0 {
            w.scale(1.0 / w_len)
        } else {
            V2::new(1.0, 0.0)
        };
        direction = V2::new(unit.z, -unit.x);
        u = unit.scale(combined_radius * inv_dt - w_len);
    }
    Line {
        point: velocity + u.scale(responsibility),
        direction,
    }
}

/// Hard wall half-plane: never approach the closest wall point faster than the
/// remaining clearance allows within `horizon`; negative clearance pushes out.
pub(crate) fn wall_line(away: V2, clearance: f32, inv_horizon: f32) -> Line {
    let n = away.normalized();
    Line {
        point: (-n).scale(clearance * inv_horizon),
        direction: V2::new(n.z, -n.x),
    }
}

fn lp1(lines: &[Line], i: usize, radius: f32, opt: V2, dir_opt: bool, result: &mut V2) -> bool {
    let line = lines[i];
    let dot = line.point.dot(line.direction);
    let disc = dot * dot + radius * radius - line.point.len_sq();
    if disc < 0.0 {
        return false; // The max-speed circle misses this line entirely.
    }
    let sqrt = disc.sqrt();
    let mut left = -dot - sqrt;
    let mut right = -dot + sqrt;
    for other in &lines[..i] {
        let denominator = det(line.direction, other.direction);
        let numerator = det(other.direction, line.point - other.point);
        if denominator.abs() <= EPSILON {
            if numerator < 0.0 {
                return false; // Parallel and pointing away: infeasible.
            }
            continue;
        }
        let t = numerator / denominator;
        if denominator >= 0.0 {
            right = right.min(t);
        } else {
            left = left.max(t);
        }
        if left > right {
            return false;
        }
    }
    let t = if dir_opt {
        if opt.dot(line.direction) > 0.0 {
            right
        } else {
            left
        }
    } else {
        // Manual clamp: left <= right holds here, and this never panics.
        line.direction.dot(opt - line.point).max(left).min(right)
    };
    *result = line.point + line.direction.scale(t);
    true
}

/// Returns `lines.len()` on success, or the index of the first infeasible line.
pub(crate) fn lp2(lines: &[Line], radius: f32, opt: V2, dir_opt: bool, result: &mut V2) -> usize {
    *result = if dir_opt {
        opt.scale(radius)
    } else if opt.len_sq() > radius * radius {
        opt.normalized().scale(radius)
    } else {
        opt
    };
    for i in 0..lines.len() {
        if det(lines[i].direction, lines[i].point - *result) > 0.0 {
            let previous = *result;
            if !lp1(lines, i, radius, opt, dir_opt, result) {
                *result = previous;
                return i;
            }
        }
    }
    lines.len()
}

/// Minimises the largest violation of agent lines while keeping the first
/// `hard` (wall) lines satisfied, starting from the first failed line.
pub(crate) fn lp3(lines: &[Line], hard: usize, begin: usize, radius: f32, result: &mut V2) {
    let mut distance = 0.0f32;
    let mut projected = Lines::new();
    for i in begin..lines.len() {
        if det(lines[i].direction, lines[i].point - *result) <= distance {
            continue;
        }
        projected.len = 0;
        for &line in &lines[..hard] {
            projected.push(line);
        }
        for j in hard..i {
            let determinant = det(lines[i].direction, lines[j].direction);
            let point = if determinant.abs() <= EPSILON {
                if lines[i].direction.dot(lines[j].direction) > 0.0 {
                    continue; // Same direction: already covered.
                }
                (lines[i].point + lines[j].point).scale(0.5)
            } else {
                lines[i].point
                    + lines[i].direction.scale(
                        det(lines[j].direction, lines[i].point - lines[j].point) / determinant,
                    )
            };
            projected.push(Line {
                point,
                direction: (lines[j].direction - lines[i].direction).normalized(),
            });
        }
        let previous = *result;
        let toward = V2::new(-lines[i].direction.z, lines[i].direction.x);
        if lp2(projected.as_slice(), radius, toward, true, result) < projected.len {
            // Only numerical error can get here; keep the previous answer.
            *result = previous;
        }
        distance = det(lines[i].direction, lines[i].point - *result);
    }
}

/// Solves one agent: walls are hard, neighbours are minimally violated if needed.
/// Returns the velocity and whether the minimal-violation fallback ran.
pub(crate) fn solve(lines: &Lines, hard: usize, max_speed: f32, preferred: V2) -> (V2, bool) {
    let mut result = V2::default();
    let slice = lines.as_slice();
    let failed = lp2(slice, max_speed, preferred, false, &mut result);
    if failed < slice.len() {
        lp3(slice, hard, failed, max_speed, &mut result);
    }
    (result, failed < slice.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_on_agents_choose_opposite_sides() {
        // Two agents approaching along X; each must deviate in Z.
        let a = V2::new(1.0, 0.0);
        let b = V2::new(-1.0, 0.0);
        let rel_pos = V2::new(2.0, 0.0);
        let mut lines = Lines::new();
        lines.push(agent_line(a, rel_pos, a - b, 1.0, 1.0 / 2.0, 60.0, 0.5));
        let (va, _) = solve(&lines, 0, 1.0, a);
        let mut lines = Lines::new();
        lines.push(agent_line(b, -rel_pos, b - a, 1.0, 1.0 / 2.0, 60.0, 0.5));
        let (vb, _) = solve(&lines, 0, 1.0, b);
        assert!(va.z.abs() > 0.05 && vb.z.abs() > 0.05);
        assert!(va.z.signum() == -vb.z.signum(), "{va:?} {vb:?}");
    }

    #[test]
    fn wall_line_blocks_approach_but_allows_sliding() {
        // Wall at z = 0, agent at z = 0.5 moving toward it and along it.
        let mut lines = Lines::new();
        lines.push(wall_line(V2::new(0.0, 0.5), 0.5, 1.0));
        let (v, _) = solve(&lines, 1, 2.0, V2::new(1.0, -2.0));
        assert!(v.z >= -0.5 - 1e-4, "{v:?}");
        assert!(v.x > 0.5);
    }
}
