use anyhow::{Context, Result, ensure};
use ara_material::{Domain, Instance, Material};
use ara_vfx::{Effect, Module};

/// Produce complete offline Slang source. Optional materials are in emitter order;
/// their domains must match the geometry outputs. Bind instance rows with
/// GpuSystem::bind_materials; texture resolution remains the asset renderer's job.
pub fn cook_source(effect: &Effect, materials: Option<&[Material]>) -> Result<String> {
    effect.validate().map_err(|d| anyhow::anyhow!("{d:?}"))?;
    ensure!(
        materials.is_none_or(|m| m.len() == effect.emitters.len()),
        "one material per emitter is required"
    );
    let mut source = include_str!("../shaders/runtime.slang").to_owned();
    let template = ara_material::ENGINE_TEMPLATE;
    let helpers = template
        .split_once("struct AraMaterialInput")
        .context("material input ABI")?
        .1;
    let helpers = format!("struct AraMaterialInput{helpers}");
    let load =
        "return buffers[NonUniformResourceIndex(draw.bufferIndex)][draw.materialIndex + offset];";
    ensure!(
        helpers.contains(load),
        "material buffer ABI changed; update VFX adaptation"
    );
    source.push_str(
        &helpers
            .replace(load, "return buffers[NonUniformResourceIndex(frame().inputs.w)].Load<float4>((root.emitter*64+offset)*16);")
            .replace("Sample(linearSampler,", "Sample(samplers[0],"),
    );
    source.push_str(&ara_vfx::generate(effect).map_err(|d| anyhow::anyhow!("{d:?}"))?);
    // Render domains come from the modules that run (disabled ones are skipped).
    let active = effect
        .resolve(&Default::default())
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    for (i, e) in active.emitters.iter().enumerate() {
        let domain = e
            .stacks
            .render
            .iter()
            .find_map(|m| match m.module {
                Module::Sprite { domain, .. }
                | Module::Ribbon { domain, .. }
                | Module::Mesh { domain, .. } => Some(domain),
                _ => None,
            })
            .context("geometry output")?;
        let default;
        let material = if let Some(materials) = materials {
            &materials[i]
        } else {
            let outputs = if domain == Domain::Surface {
                "let tint : ColorLinear = linear(1,1,1); let rough : float = 0.8; output baseColor = tint; output roughness = rough;"
            } else {
                "let tint : ColorLinear = linear(1,1,1); output color = tint;"
            };
            default = ara_material::dsl::parse(
                &format!(
                    "material Vfx{i} : {} budget 1 {{ {outputs} }}",
                    domain.name()
                ),
                &format!("gr_vfx{i}"),
            )
            .map_err(|d| anyhow::anyhow!("{d:?}"))?;
            &default
        };
        ensure!(
            material.domain == domain,
            "emitter/material domain mismatch"
        );
        let generated = ara_material::generate(material, &Instance::default())
            .map_err(|d| anyhow::anyhow!("{d:?}"))?;
        ensure!(
            generated
                .parameters
                .iter()
                .map(|p| p.offset + p.rows)
                .max()
                .unwrap_or(0)
                <= 64,
            "VFX material exceeds 64 float4 uniform rows"
        );
        let body = generated
            .source
            .strip_prefix(template)
            .context("material template mismatch")?
            .split("// Offline verification entry.")
            .next()
            .context("material evaluation body")?;
        source.push_str(&body.replace("ara_evaluate(", &format!("evaluate{i}(")));
    }
    source.push_str(
        "AraSurface evaluateMaterial(uint emitter,AraMaterialInput input){switch(emitter){\n",
    );
    for i in 0..effect.emitters.len() {
        source.push_str(&format!("case {i}u:return evaluate{i}(input);\n"));
    }
    source.push_str("}return evaluate0(input);}\n");
    source.push_str(include_str!("../shaders/kernels.slang"));
    source.push_str(include_str!("../shaders/render.slang"));
    Ok(source)
}
