fn main() {
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").warnings(false);
    for module in ["Recast", "Detour", "DetourCrowd", "DetourTileCache"] {
        build.include(format!("vendor/{module}/Include"));
        let directory = format!("vendor/{module}/Source");
        let mut sources: Vec<_> = std::fs::read_dir(&directory)
            .expect("vendored Recast source directory exists")
            .map(|entry| entry.expect("source entry is readable").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "cpp"))
            .collect();
        sources.sort();
        build.files(sources);
        println!("cargo:rerun-if-changed=vendor/{module}");
    }
    // Virtual query filters let the shim block off-mesh links at runtime. Every
    // Detour translation unit sees the same definition (one cc build).
    build.define("DT_VIRTUAL_QUERYFILTER", "1");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flag("/fp:strict").flag("/EHsc");
    } else {
        build.flag("-fno-fast-math").flag("-ffp-contract=off");
    }
    build.file("shim/bake.cpp").file("shim/runtime.cpp");
    build.compile("ara_recast");
    println!("cargo:rerun-if-changed=shim");
}
