use anyhow::Result;
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
    Ok(())
}
