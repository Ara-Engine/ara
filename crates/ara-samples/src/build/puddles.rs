//! Original procedural ripple art and native water/wet-rim surface bindings.
use super::{Ctx, materials::graph, place::Models};
use anyhow::{Context, Result};
use glam::Vec3;
use serde_json::json;

/// Generates a periodic tangent-space normal from the analytic slope of three waves.
fn ripple_normal() -> image::RgbImage {
    const SIZE: u32 = 256;
    image::RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let u = (x as f32 + 0.5) / SIZE as f32;
        let v = (y as f32 + 0.5) / SIZE as f32;
        let (mut du, mut dv) = (0.0, 0.0);
        for (kx, ky, amplitude, phase) in [
            (2.0, 1.0, 0.006, 0.3),
            (-1.0, 3.0, 0.004, 1.7),
            (5.0, 4.0, 0.0012, 0.8),
        ] {
            let slope = amplitude
                * std::f32::consts::TAU
                * (std::f32::consts::TAU * (kx * u + ky * v) + phase).cos();
            du += kx * slope;
            dv += ky * slope;
        }
        let normal = Vec3::new(-du, -dv, 1.0).normalize();
        image::Rgb(
            normal
                .to_array()
                .map(|c| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8),
        )
    })
}

/// Binds the same water/rim graph pair to each generated puddle variant.
pub(super) fn materials(ctx: &Ctx, models: &mut Models) -> Result<()> {
    let path = "assets/kit/puddle_ripples.png";
    ripple_normal().save(ctx.root.join(path))?;
    ctx.sidecar(path, "as_puddleripples", json!({"usage":"normal"}))?;
    let water = graph(
        ctx,
        "puddlewater",
        "material PuddleWater : surface budget 1 {
            param rippleMap : TextureNormal = @assets/kit/puddle_ripples.png#as_puddleripples;
            let uv0 : vec2 = uv();
            let repeat : vec2 = vec2(4, 4);
            let coords : vec2 = mul(uv0, repeat);
            let ripple : vec3 = sampleNormal(rippleMap, coords);
            let strength : float = 0.15;
            let normal : vec3 = normalStrength(ripple, strength);
            let color : ColorLinear = linear(0.035, 0.032, 0.03);
            let roughness : float = 0.08;
            output baseColor = color;
            output roughness = roughness;
            output normal = normal;
        }"
        .into(),
    )?;
    let rim = graph(
        ctx,
        "puddlewetedge",
        "material WetEdge : surface budget 1 {
            let color : ColorLinear = linear(0.035, 0.032, 0.03);
            let roughness : float = 0.65;
            output baseColor = color;
            output roughness = roughness;
        }"
        .into(),
    )?;
    for name in ["puddle_a", "puddle_b", "puddle_c"] {
        models.0.get_mut(name).context("puddle model")?.renderer["materialOverrides"] =
            json!([{"primitive":0,"material":rim},{"primitive":1,"material":water}]);
    }
    Ok(())
}
