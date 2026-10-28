use anyhow::Result;
use ara_shader_cook::write_dependency_index;
use std::{fs, path::Path};

#[test]
fn source_index_tracks_cross_package_includes_and_conservative_modules_without_sdk() -> Result<()> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-shader-cook/tests");
    fs::create_dir_all(&base)?;
    let temp = tempfile::tempdir_in(base)?;
    let root = temp.path();
    fs::create_dir_all(root.join("shaders"))?;
    fs::write(root.join("common.slangh"), "// ABI header\n")?;
    fs::write(
        root.join("shaders/main.slang"),
        "#include \"../common.slangh\"\n",
    )?;
    let manifest = root.join("shaders/pipelines.json");
    fs::write(&manifest,br#"{"main":{"source":"main.slang","entries":{"main":"compute"},"state":{"color_formats":[],"depth_format":null,"vertex_stride":0,"depth_test":false,"depth_write":false,"blend":false}}}"#)?;
    let output = root.join("cooked");
    write_dependency_index(&manifest, &output, None)?;
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join(".ara-shader-dependencies.json"))?)?;
    assert_eq!(index["files"].as_array().map(Vec::len), Some(2));
    assert_eq!(index["conservative"], false);
    fs::write(root.join("shaders/unused.slang"), "unfinished source")?;
    write_dependency_index(&manifest, &output, None)?;
    assert_eq!(
        index,
        serde_json::from_slice::<serde_json::Value>(&fs::read(
            output.join(".ara-shader-dependencies.json")
        )?)?
    );
    fs::write(root.join("shaders/main.slang"), "import dynamic_module;\n")?;
    write_dependency_index(&manifest, &output, None)?;
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join(".ara-shader-dependencies.json"))?)?;
    assert_eq!(index["conservative"], true);
    Ok(())
}
