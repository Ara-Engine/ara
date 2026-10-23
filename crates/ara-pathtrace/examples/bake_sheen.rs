//! Offline Charlie directional-albedo quadrature; normal builds consume the baked asset.
use std::{f64::consts::PI, path::Path};

fn lambda(c: f64, alpha: f64) -> f64 {
    let k = (1. - alpha).powi(2);
    let mix = |a: f64, b: f64| a + (b - a) * k;
    let l = |x: f64| {
        mix(21.5473, 25.3245)
            / (1. + mix(3.82987, 3.32435) * x.max(1e-5).powf(mix(0.19823, 0.16801)))
            + mix(-1.9776, -1.27393) * x
            + mix(-4.32054, -4.85967)
    };
    (if c < 0.5 {
        l(c)
    } else {
        2. * l(0.5) - l(1. - c)
    })
    .exp()
}
fn main() -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    for y in 0..32 {
        let roughness = 0.1 + 0.9 * f64::from(y) / 31.;
        let alpha = roughness * roughness;
        for x in 0..32 {
            let nv = (f64::from(x) / 31.).max(1e-4);
            let v = glam::DVec3::new((1. - nv * nv).sqrt(), 0., nv);
            let mut energy = 0.;
            let lv = lambda(nv, alpha);
            for mu in 0..64 {
                let nl = (f64::from(mu) + 0.5) / 64.;
                let radial = (1. - nl * nl).sqrt();
                let ll = lambda(nl, alpha);
                for azimuth in 0..64 {
                    let phi = (f64::from(azimuth) + 0.5) * 2. * PI / 64.;
                    let light = glam::DVec3::new(radial * phi.cos(), radial * phi.sin(), nl);
                    let h = (v + light).normalize();
                    let d =
                        (2. + 1. / alpha) * (1. - h.z * h.z).max(0.).powf(0.5 / alpha) / (2. * PI);
                    energy += d / (4. * nv * (1. + lv + ll));
                }
            }
            // A small conservative quadrature/interpolation allowance prevents gain
            // between grid points without the old unit-albedo darkening.
            let e = (energy * (2. * PI / 4096.) * 1.01 + 0.002).clamp(0., 1.) as f32;
            for value in [e, 0., 0., 1.] {
                bytes.extend(value.to_le_bytes());
            }
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ara-render-fidelity/shaders/sheen-albedo.bin");
    std::fs::write(path, bytes)?;
    Ok(())
}
