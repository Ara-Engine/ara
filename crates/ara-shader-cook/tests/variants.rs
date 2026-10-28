use anyhow::Result;
use ara_gpu::cooked::CookedPipeline;
use ara_shader_cook::{
    Toolchain, cook, cook_with_variant,
    variant::{Variant, promote},
};
use std::path::Path;

#[test]
#[cfg_attr(target_os = "macos", ignore = "PC is the only shader cook host")]
fn permutations_have_distinct_artifacts_and_never_mutate_baseline_before_promotion() -> Result<()> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-shader-cook/variants");
    std::fs::create_dir_all(&base)?;
    let temporary = tempfile::tempdir_in(base)?;
    let root = temporary.path();
    std::fs::write(
        root.join("test.slang"),
        "#ifndef TEST_VALUE\n#define TEST_VALUE 1.0\n#endif\n[[vk::binding(1,0)]] RWStructuredBuffer<float> result:register(u0,space2); [shader(\"compute\")] [numthreads(1,1,1)] void computeMain(uint3 t:SV_DispatchThreadID){result[t.x]=TEST_VALUE;}",
    )?;
    let manifest = root.join("pipelines.json");
    let mut input = serde_json::json!({"a":{"source":"test.slang","entries":{"computeMain":"compute"},"state":{"color_formats":[],"depth_format":null,"vertex_stride":0,"depth_test":false,"depth_write":false,"blend":false}}});
    input["b"] = input["a"].clone();
    input["b"]["defines"] = serde_json::json!({"TEST_VALUE":"3.0"});
    std::fs::write(&manifest, serde_json::to_vec(&input)?)?;
    let original = std::fs::read(&manifest)?;
    let tools = Toolchain::discover();
    let baseline = root.join("baseline");
    assert_eq!(cook(&manifest, &baseline, &tools)?, 2);
    let load = |path: &Path| -> Result<CookedPipeline> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    };
    let a = load(&baseline.join("a.json"))?;
    let b = load(&baseline.join("b.json"))?;
    std::fs::write(
        root.join("unfinished-experiment.slang"),
        "invalid syntax !!!",
    )?;
    assert_eq!(cook(&manifest, &baseline, &tools)?, 0);
    assert_eq!(
        a.key,
        load(&baseline.join("a.json"))?.key,
        "an unfinished sibling cannot change baseline provenance"
    );
    assert_ne!(
        a.stages[0].spirv, b.stages[0].spirv,
        "same entry names must not alias"
    );
    assert_ne!(a.stages[0].spirv_hash, b.stages[0].spirv_hash);
    let variant: Variant = serde_json::from_value(
        serde_json::json!({"pipelines":{"a":{"defines":{"TEST_VALUE":"2.0"}}}}),
    )?;
    let experiment = root.join("experiment");
    cook_with_variant(&manifest, &experiment, &tools, Some(&variant))?;
    let changed = load(&experiment.join("a.json"))?;
    assert_ne!(a.stages[0].spirv_hash, changed.stages[0].spirv_hash);
    assert_eq!(
        b.stages[0].spirv_hash,
        load(&experiment.join("b.json"))?.stages[0].spirv_hash
    );
    assert_eq!(std::fs::read(&manifest)?, original);
    assert_eq!(
        a.stages[0].spirv_hash,
        load(&baseline.join("a.json"))?.stages[0].spirv_hash
    );
    promote(&manifest, &variant)?;
    assert_ne!(std::fs::read(&manifest)?, original);
    let promoted = root.join("promoted");
    cook(&manifest, &promoted, &tools)?;
    assert_eq!(
        changed.stages[0].spirv_hash,
        load(&promoted.join("a.json"))?.stages[0].spirv_hash
    );
    Ok(())
}
