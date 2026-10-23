//! Compiles the vendored Jolt sources plus Ara's C shim into one static library with the
//! `cc` crate. No CMake: Jolt is a flat list of .cpp files, and keeping the defines in one
//! place guarantees the shim and Jolt agree on every ABI-affecting switch.
//!
//! Source: `../vendor/JoltPhysics` (fetched by `setup.ps1`), or `ARA_JOLT_SOURCE`.

use std::env;
use std::path::{Path, PathBuf};

/// Files Jolt only compiles with ENABLE_OBJECT_STREAM (see Jolt/Jolt.cmake). We leave the
/// object stream out: Ara has its own serialization, and it keeps the binary smaller.
const OBJECT_STREAM_ONLY: &[&str] = &[
    "ObjectStream.cpp",
    "ObjectStreamBinaryIn.cpp",
    "ObjectStreamBinaryOut.cpp",
    "ObjectStreamIn.cpp",
    "ObjectStreamOut.cpp",
    "ObjectStreamTextIn.cpp",
    "ObjectStreamTextOut.cpp",
    "TypeDeclarations.cpp",
];

fn collect_cpp(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_cpp(&path, out);
        } else if path.extension().is_some_and(|e| e == "cpp") {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !OBJECT_STREAM_ONLY.contains(&name) {
                out.push(path);
            }
        }
    }
}

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let root = env::var_os("ARA_JOLT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../vendor/JoltPhysics"));
    let jolt_dir = root.join("Jolt");
    assert!(
        jolt_dir.join("Jolt.h").exists(),
        "Jolt source not found at {}. Run setup.ps1 first.",
        root.display()
    );

    let deterministic = env::var_os("CARGO_FEATURE_CROSS_PLATFORM_DETERMINISTIC").is_some();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let msvc = env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc");

    let mut files = Vec::new();
    collect_cpp(&jolt_dir, &mut files);

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .include(&root)
        .include(manifest.join("shim"))
        .warnings(false)
        .files(&files)
        .file(manifest.join("shim/ara_jolt.cpp"));

    if env::var("PROFILE").is_ok_and(|p| p == "release") {
        build.define("NDEBUG", None);
    }
    if deterministic {
        build.define("JPH_CROSS_PLATFORM_DETERMINISTIC", None);
    }

    // Instruction sets: Jolt's CMake defaults for x86-64 (AVX2 tier). FMA changes rounding,
    // so Jolt disables it in cross-platform deterministic builds; we do the same.
    if target_arch == "x86_64" {
        for def in [
            "JPH_USE_SSE4_1",
            "JPH_USE_SSE4_2",
            "JPH_USE_AVX",
            "JPH_USE_AVX2",
            "JPH_USE_LZCNT",
            "JPH_USE_TZCNT",
            "JPH_USE_F16C",
        ] {
            build.define(def, None);
        }
        if !deterministic {
            build.define("JPH_USE_FMADD", None);
        }
        if msvc {
            build.flag("/arch:AVX2");
        } else {
            for f in ["-mavx2", "-mbmi", "-mpopcnt", "-mlzcnt", "-mf16c"] {
                build.flag(f);
            }
            if !deterministic {
                build.flag("-mfma");
            }
        }
    }

    // Floating point model, per Jolt/Build/CMakeLists.txt and Docs/Architecture.md.
    if msvc {
        build.flag(if deterministic {
            "/fp:precise"
        } else {
            "/fp:fast"
        });
        build.flag("/fp:except-");
        build.flag("/GR-");
        build.flag("/Zc:__cplusplus");
        build.flag("/Gy");
        build.flag("/GS-");
    } else {
        build.flag("-fno-rtti");
        if deterministic {
            build.flag("-ffp-model=precise");
            build.flag("-ffp-contract=off");
        }
    }

    build.compile("ara_jolt");

    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", jolt_dir.display());
    println!("cargo:rerun-if-env-changed=ARA_JOLT_SOURCE");
}
