use anyhow::{Context, Result};
use ara_shader_cook::{Toolchain, cook};
use std::path::PathBuf;
#[test]
fn cook_reuses_intact_artifacts_and_recovers_corruption() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path();
    let source = root.join("test.slang");
    let manifest = root.join("pipelines.json");
    let output = root.join("cooked");
    let valid = "RWStructuredBuffer<float> result : register(u0); [shader(\"compute\")] [numthreads(1,1,1)] void computeMain(uint3 t:SV_DispatchThreadID) {result[t.x]=1.;}";
    std::fs::write(&source, valid)?;
    std::fs::write(
        &manifest,
        r#"{"test":{"source":"test.slang","entries":{"computeMain":"compute"},"state":{"color_formats":[],"depth_format":null,"vertex_stride":0,"depth_test":false,"depth_write":false,"blend":false}}}"#,
    )?;
    let sdk = std::env::var_os("VULKAN_SDK")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/VulkanSDK/1.4.363.0"));
    let bin = sdk.join("Bin");
    let tools = Toolchain {
        slangc: bin.join(format!("slangc{}", std::env::consts::EXE_SUFFIX)),
        spirv_val: bin.join(format!("spirv-val{}", std::env::consts::EXE_SUFFIX)),
    };
    assert_eq!(cook(&manifest, &output, &tools)?, 1);
    assert_eq!(cook(&manifest, &output, &tools)?, 0);
    let cooked: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(output.join("test.json"))?)?;
    std::fs::write(output.join(&cooked.stages[0].spirv), b"corrupt")?;
    assert_eq!(cook(&manifest, &output, &tools)?, 1);
    let good_manifest = std::fs::read(output.join("test.json"))?;
    let good_spirv = std::fs::read(output.join(&cooked.stages[0].spirv))?;
    std::fs::write(&source, "invalid shader syntax")?;
    assert!(cook(&manifest, &output, &tools).is_err());
    assert_eq!(good_manifest, std::fs::read(output.join("test.json"))?);
    assert_eq!(
        good_spirv,
        std::fs::read(output.join(&cooked.stages[0].spirv))?
    );
    // Independent source changes reuse the other pipeline's SPIR-V while publishing one cohort.
    std::fs::write(&source, valid)?;
    std::fs::write(
        root.join("other.slang"),
        valid.replace("computeMain", "otherMain"),
    )?;
    let first: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest)?)?;
    let mut combined = first.as_object().context("manifest object")?.clone();
    let mut other = combined["test"].clone();
    other["source"] = "other.slang".into();
    other["entries"] = serde_json::json!({"otherMain":"compute"});
    combined.insert("other".into(), other);
    std::fs::write(&manifest, serde_json::to_vec(&combined)?)?;
    cook(&manifest, &output, &tools)?;
    let other_before: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(output.join("other.json"))?)?;
    std::fs::write(&source, valid.replace("=1.", "=2."))?;
    assert_eq!(cook(&manifest, &output, &tools)?, 1);
    let other_after: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(output.join("other.json"))?)?;
    let test_after: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(output.join("test.json"))?)?;
    assert_eq!(other_before.stages[0].spirv, other_after.stages[0].spirv);
    assert_eq!(other_after.key, test_after.key);
    let good = std::fs::read(output.join("test.json"))?;
    std::fs::write(&source, valid.replace("=1.", "=3."))?;
    std::fs::write(root.join("other.slang"), "invalid shader")?;
    assert!(cook(&manifest, &output, &tools).is_err());
    assert_eq!(good, std::fs::read(output.join("test.json"))?);
    Ok(())
}
