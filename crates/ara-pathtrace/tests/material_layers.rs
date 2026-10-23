use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_pathtrace::{Reference, scenes};
use ara_render_fidelity::{PostSettings, material::MaterialDetail};

#[test]
fn layered_white_furnace_has_no_energy_gain() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/material-test-cache"))?;
    for detail in [
        MaterialDetail {
            clearcoat: 1.,
            coat_roughness: 0.2,
            ..Default::default()
        },
        MaterialDetail {
            sheen: [0.3; 3],
            sheen_roughness: 0.6,
            ..Default::default()
        },
        MaterialDetail {
            transmission: 0.4,
            ..Default::default()
        },
    ] {
        let mut scene = scenes::scene("furnace", 0);
        scene.materials[0].base_color = [1.; 4];
        detail.apply(&mut scene.materials[0])?;
        let mut reference =
            Reference::new(&mut gpu, &scene, [64, 36], 1024, 8, PostSettings::default())?;
        reference.converge(&mut gpu, &scenes::camera())?;
        let data = reference.radiance(&mut gpu)?;
        let samples = data
            .iter()
            .enumerate()
            .filter(|(i, _)| (27..37).contains(&(i % 64)) && (13..23).contains(&(i / 64)))
            .map(|(_, p)| f64::from(p[0] + p[1] + p[2]) / 3.)
            .collect::<Vec<_>>();
        let average = samples.iter().sum::<f64>() / samples.len() as f64;
        println!("{detail:?}: furnace energy {average}");
        if detail.sheen.iter().any(|c| *c > 0.) {
            ensure!(
                average > 0.95,
                "Sheen loses white-furnace energy: {average}"
            );
        }
        ensure!(
            average > 0.5 && average < 1.04,
            "Layered furnace energy {average}, {detail:?}"
        );
        reference.destroy(&mut gpu)?;
    }
    gpu.assert_clean()?;
    Ok(())
}
