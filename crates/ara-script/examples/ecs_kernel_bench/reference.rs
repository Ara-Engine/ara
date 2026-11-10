//! Independent scalar oracle for S8's twelve-float records; never runs in a timed tick.
pub const DT: f32 = 1.0 / 60.0;
pub type Record = [[f32; 4]; 3];

/// Integer-derived initial state, in Position/Velocity/Rotation order.
pub fn initial(id: u32) -> Record {
    let unit = |salt: u32| {
        let mut x = id ^ salt;
        x = (x ^ (x >> 16)).wrapping_mul(0x7feb_352d);
        x = (x ^ (x >> 15)).wrapping_mul(0x846c_a68b);
        ((x ^ (x >> 16)) & 65535) as f32 / 65536.0
    };
    [
        [
            unit(2_891_336_453) * 64.0 - 32.0,
            unit(1_181_783_497) * 64.0 - 32.0,
            unit(425_950_093) * 64.0 - 32.0,
            1.0,
        ],
        [
            unit(97) * 2.0 - 1.0,
            unit(193) * 2.0 - 1.0,
            unit(389) * 2.0 - 1.0,
            0.0,
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// W1 preserves the reference's operation grouping and IEEE sqrt/division.
pub fn transform(record: &mut Record) {
    let [p, v, q] = record;
    for axis in 0..4 {
        p[axis] += v[axis] * DT;
    }
    let a = v.map(|value| value * 0.1);
    let d = [
        a[0] * q[3] + a[1] * q[2] - a[2] * q[1],
        a[1] * q[3] + a[2] * q[0] - a[0] * q[2],
        a[2] * q[3] + a[0] * q[1] - a[1] * q[0],
        -((a[0] * q[0] + a[1] * q[1]) + a[2] * q[2]),
    ];
    for axis in 0..4 {
        q[axis] += d[axis] * (0.5 * DT);
    }
    let norm = ((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3];
    let inv = 1.0 / norm.sqrt();
    for value in q {
        *value *= inv;
    }
}

/// W3 returns whether this tick bounced, for complete bounce coverage accounting.
pub fn particle(record: &mut Record) -> bool {
    let [p, v, _] = record;
    for axis in 0..4 {
        v[axis] = (v[axis] + [0.0, -9.81, 0.0, 0.0][axis] * DT) * 0.999;
        p[axis] += v[axis] * DT;
    }
    if p[1] < 0.0 {
        p[1] = -p[1];
        v[1] = v[1].abs() * 0.8;
        true
    } else {
        false
    }
}
