//! Builds the committed native shim; no generators or downloads.
fn main() {
    for file in [
        "shim/context.cpp",
        "shim/state.h",
        "shim/prepare.inc",
        "shim/run.inc",
        "vendor/simplifier.cpp",
        "vendor/meshoptimizer.h",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    let mut build = cc::Build::new();
    build.cpp(true).file("shim/context.cpp").opt_level(3);
    if std::env::var("DEBUG").is_ok_and(|v| v != "true") {
        build.define("NDEBUG", None);
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "msvc") {
        build.flag("/EHsc");
    } else {
        build.flag("-std=c++11");
    }
    build.compile("ara_meshopt_prepared");
}
