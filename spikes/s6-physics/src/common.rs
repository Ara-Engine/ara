//! Shared helpers: math aliases, the state hash, timing statistics and the build flavour.

pub use rapier3d::glamx::glam::{Quat, Vec3};
use serde::Serialize;
use std::time::Instant;

/// Fixed simulation step used by every scenario.
pub const DT: f32 = 1.0 / 60.0;
pub const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

/// Which engine a run used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Jolt,
    Rapier,
}

impl EngineKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "jolt" => Some(Self::Jolt),
            "rapier" => Some(Self::Rapier),
            _ => None,
        }
    }
}

/// Build flavour, recorded next to every number so runs from different builds are never mixed.
#[derive(Clone, Debug, Serialize)]
pub struct Flavor {
    pub deterministic_build: bool,
    pub jolt_cross_platform_deterministic: bool,
    pub rapier_parallel: bool,
    pub rapier_simd8: bool,
    pub rapier_enhanced_determinism: bool,
    /// Rust code compiled with AVX2/FMA (RUSTFLAGS=-C target-cpu=x86-64-v3). Jolt's C++ is
    /// always compiled for AVX2.
    pub rust_avx2: bool,
}

impl Flavor {
    pub fn current() -> Self {
        // SAFETY: a pure function with no arguments.
        let jolt_cpd = unsafe { ara_jolt_sys::ara_cross_platform_deterministic() } == 1;
        Self {
            deterministic_build: cfg!(feature = "det"),
            jolt_cross_platform_deterministic: jolt_cpd,
            rapier_parallel: cfg!(feature = "parallel"),
            rapier_simd8: cfg!(feature = "simd8"),
            rapier_enhanced_determinism: cfg!(feature = "det"),
            rust_avx2: cfg!(target_feature = "avx2"),
        }
    }

    /// Short tag used in evidence directory names, e.g. `fast-par-simd8-avx2`.
    pub fn tag(&self) -> String {
        let mut t = String::from(if self.deterministic_build {
            "det"
        } else {
            "fast"
        });
        t += if self.rapier_parallel { "-par" } else { "-st" };
        if self.rapier_simd8 {
            t += "-simd8";
        }
        if self.rust_avx2 {
            t += "-avx2";
        }
        t
    }
}

/// FNV-1a over raw bytes. Stable across platforms and runs (unlike `std::hash`).
#[derive(Clone, Copy)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub fn vec3(&mut self, v: Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
    pub fn quat(&mut self, q: Quat) {
        self.f32(q.x);
        self.f32(q.y);
        self.f32(q.z);
        self.f32(q.w);
    }
}

/// Percentile summary of a set of samples in milliseconds.
#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub samples: usize,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

impl Stats {
    /// Nearest-rank percentiles.
    pub fn from_ms(samples: &[f64]) -> Self {
        let mut s = samples.to_vec();
        s.sort_by(f64::total_cmp);
        let rank = |p: f64| {
            if s.is_empty() {
                return 0.0;
            }
            let idx = ((p / 100.0) * s.len() as f64).ceil() as usize;
            s[idx.clamp(1, s.len()) - 1]
        };
        Self {
            samples: s.len(),
            p50: rank(50.0),
            p95: rank(95.0),
            max: s.last().copied().unwrap_or(0.0),
            mean: if s.is_empty() {
                0.0
            } else {
                s.iter().sum::<f64>() / s.len() as f64
            },
        }
    }
}

/// Milliseconds elapsed since `start`.
pub fn ms_since(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Small deterministic generator for scene layouts (same values for both engines).
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}
