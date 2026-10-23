//! One-time canonical shader-recipe migration.
fn main() -> anyhow::Result<()> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ara-render-fidelity/shaders/pipelines.json");
    let mut recipes: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    for (name, entry, source) in [
        ("transmittance", "transmittanceMain", "atmosphere.slang"),
        ("multiscatter", "multiScatterMain", "atmosphere.slang"),
        ("skyview", "skyViewMain", "atmosphere.slang"),
        ("fog", "fogMain", "fog.slang"),
        ("fog_raster", "fogMain", "fog-raster.slang"),
        ("diagnostic", "diagnosticMain", "diagnostic.slang"),
        (
            "primary_atmosphere",
            "primaryAtmosphereMain",
            "transport-atmosphere.slang",
        ),
        (
            "hybrid_atmosphere",
            "hybridAtmosphereMain",
            "transport-atmosphere.slang",
        ),
        (
            "reference_atmosphere",
            "referenceAtmosphereMain",
            "transport-atmosphere.slang",
        ),
        (
            "ddgi_atmosphere",
            "ddgiAtmosphereMain",
            "ddgi-atmosphere.slang",
        ),
        (
            "primary_layers",
            "primaryLayersMain",
            "transport-layers.slang",
        ),
        (
            "hybrid_layers",
            "hybridLayersMain",
            "transport-layers.slang",
        ),
        (
            "reference_layers",
            "referenceLayersMain",
            "transport-layers.slang",
        ),
        ("ddgi_layers", "ddgiLayersMain", "ddgi-layers.slang"),
    ] {
        let mut recipe = recipes["bloom"].to_owned();
        let entry = if name == "fog_raster" {
            "fogRasterMain"
        } else {
            entry
        };
        recipe["entries"] = serde_json::json!({entry:"compute"});
        recipe["source"] = source.into();
        recipes[name] = recipe;
    }
    ara_gpu::cooked::write_json(&path, &recipes)?;
    Ok(())
}
