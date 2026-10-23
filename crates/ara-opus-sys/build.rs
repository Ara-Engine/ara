//! Compiler-only build of the pinned, vendored floating-point Opus implementation.
use std::{fs, path::Path};

fn sources(manifest: &str, groups: &[&str]) -> Vec<String> {
    let text = fs::read_to_string(manifest).expect("vendored source manifest exists");
    let mut selected = false;
    let mut files = Vec::new();
    for line in text.lines() {
        if let Some((name, _)) = line.split_once('=') {
            selected = groups.contains(&name.trim());
        } else if selected {
            let file = line.trim().trim_end_matches('\\').trim();
            if file.ends_with(".c") {
                files.push(file.to_owned());
            }
        }
    }
    files
}

fn main() {
    let root = Path::new("vendor/opus-1.5.2");
    let mut build = cc::Build::new();
    build.warnings(false).opt_level(3);
    for dir in ["", "include", "celt", "silk", "silk/float"] {
        build.include(root.join(dir));
    }
    // Portable scalar reference code: no assembler, generated headers, CPU probes or tools.
    // Each target's compiler supplies alloca; the shipped headers supply integer types.
    build
        .define("OPUS_BUILD", None)
        .define("USE_ALLOCA", None)
        .define("ENABLE_HARDENING", None)
        .define("OPUS_DISABLE_INTRINSICS", None)
        .define("PACKAGE_VERSION", "\"1.5.2\"");
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        build.define("HAVE_ALLOCA_H", None);
    }
    for (manifest, groups) in [
        (
            "opus_sources.mk",
            &["OPUS_SOURCES", "OPUS_SOURCES_FLOAT"][..],
        ),
        ("celt_sources.mk", &["CELT_SOURCES"][..]),
        (
            "silk_sources.mk",
            &["SILK_SOURCES", "SILK_SOURCES_FLOAT"][..],
        ),
    ] {
        for file in sources(&root.join(manifest).to_string_lossy(), groups) {
            build.file(root.join(file));
        }
    }
    build.file("shim.c").compile("ara_opus");
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        println!("cargo:rustc-link-lib=m");
    }
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=shim.c");
}
