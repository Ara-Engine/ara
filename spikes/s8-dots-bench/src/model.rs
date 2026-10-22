use glam::{Vec3, Vec4};

pub const DT: f32 = 1.0 / 60.0;
pub const CELLS: usize = 16 * 16 * 16;

/// Integer-derived input shared bit-for-bit with the C# implementations.
pub fn initial(id: u32) -> (Vec4, Vec4, Vec4) {
    let unit = |salt: u32| {
        let mut x = id ^ salt;
        x = (x ^ (x >> 16)).wrapping_mul(0x7feb_352d);
        x = (x ^ (x >> 15)).wrapping_mul(0x846c_a68b);
        ((x ^ (x >> 16)) & 65535) as f32 / 65536.0
    };
    (
        Vec4::new(
            unit(2_891_336_453) * 64.0 - 32.0,
            unit(1_181_783_497) * 64.0 - 32.0,
            unit(425_950_093) * 64.0 - 32.0,
            1.0,
        ),
        Vec4::new(
            unit(97) * 2.0 - 1.0,
            unit(193) * 2.0 - 1.0,
            unit(389) * 2.0 - 1.0,
            0.0,
        ),
        Vec4::W,
    )
}

/// W1: integrate translation and a normalized quaternion Euler step.
pub fn transform(p: &mut Vec4, v: Vec4, q: &mut Vec4) {
    *p += v * DT;
    let a = v * 0.1;
    let d = Vec4::new(
        a.x * q.w + a.y * q.z - a.z * q.y,
        a.y * q.w + a.z * q.x - a.x * q.z,
        a.z * q.w + a.x * q.y - a.y * q.x,
        -((a.x * q.x + a.y * q.y) + a.z * q.z),
    );
    *q += d * (0.5 * DT);
    let norm = ((q.x * q.x + q.y * q.y) + q.z * q.z) + q.w * q.w;
    *q *= 1.0 / norm.sqrt();
}

/// W3: float4 gravity, linear drag, integration, and a y=0 bounce.
pub fn particle(p: &mut Vec4, v: &mut Vec4) {
    *v = (*v + Vec4::new(0.0, -9.81, 0.0, 0.0) * DT) * 0.999;
    *p += *v * DT;
    if p.y < 0.0 {
        p.y = -p.y;
        v.y = v.y.abs() * 0.8;
    }
}

/// A bounded spatial hash with collision-free keys and stable-ID bucket order.
pub struct Grid {
    pub offsets: Vec<i32>,
    pub ids: Vec<i32>,
    pub cursor: Vec<i32>,
}

pub fn cell(p: Vec4) -> [i32; 3] {
    [p.x, p.y, p.z].map(|v| (((v + 32.0) * 0.25).floor() as i32).clamp(0, 15))
}

pub fn key(c: [i32; 3]) -> usize {
    (c[0] + 16 * (c[1] + 16 * c[2])) as usize
}

impl Grid {
    pub fn new(n: usize) -> Self {
        Self {
            offsets: vec![0; CELLS + 1],
            ids: vec![0; n],
            cursor: vec![0; CELLS],
        }
    }

    /// Count, prefix sum, then scatter ascending IDs; this work is timed.
    pub fn build(&mut self, positions: &[Vec4]) {
        self.offsets.fill(0);
        for &p in positions {
            self.offsets[key(cell(p)) + 1] += 1;
        }
        for c in 0..CELLS {
            self.offsets[c + 1] += self.offsets[c];
        }
        self.cursor.copy_from_slice(&self.offsets[..CELLS]);
        for (id, &p) in positions.iter().enumerate() {
            let c = key(cell(p));
            self.ids[self.cursor[c] as usize] = id as i32;
            self.cursor[c] += 1;
        }
    }

    /// W2: read a Jacobi snapshot; accumulate neighbours in a defined order.
    pub fn boid(&self, id: usize, positions: &[Vec4], velocities: &[Vec4]) -> (Vec4, Vec4) {
        let p = positions[id];
        let mut v = velocities[id];
        let mut separation = Vec3::ZERO;
        let mut alignment = Vec3::ZERO;
        let mut cohesion = Vec3::ZERO;
        let mut count = 0;
        let c = cell(p);
        for z in (c[2] - 1).max(0)..=(c[2] + 1).min(15) {
            for y in (c[1] - 1).max(0)..=(c[1] + 1).min(15) {
                for x in (c[0] - 1).max(0)..=(c[0] + 1).min(15) {
                    let k = key([x, y, z]);
                    for index in self.offsets[k]..self.offsets[k + 1] {
                        let j = self.ids[index as usize] as usize;
                        let d = (p - positions[j]).truncate();
                        let d2 = (d.x * d.x + d.y * d.y) + d.z * d.z;
                        if j != id && d2 < 16.0 && d2 > 0.0 {
                            separation += d / d2.max(0.01);
                            alignment += velocities[j].truncate();
                            cohesion += positions[j].truncate();
                            count += 1;
                        }
                    }
                }
            }
        }
        if count > 0 {
            let inv = 1.0 / count as f32;
            let acceleration = separation * 0.02
                + (alignment * inv - v.truncate()) * 0.05
                + (cohesion * inv - p.truncate()) * 0.01;
            v += (acceleration * DT).extend(0.0);
        }
        let speed2 = (v.x * v.x + v.y * v.y) + v.z * v.z;
        if speed2 > 16.0 {
            v *= 4.0 / speed2.sqrt();
        }
        let mut next = p + v * DT;
        for axis in 0..3 {
            if next[axis] >= 32.0 {
                next[axis] -= 64.0;
            }
            if next[axis] < -32.0 {
                next[axis] += 64.0;
            }
        }
        (next, v)
    }
}
