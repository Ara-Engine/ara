//! Compiles the vendored Jolt sources plus Ara's C shim into one static library with the
//! `cc` crate. No CMake: Jolt is a flat list of .cpp files, and keeping the defines in one
//! place guarantees the shim and Jolt agree on every ABI-affecting switch.
//!
//! Source: committed `vendor/Jolt`, pinned by `vendor/REVISION`.

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
    let root = manifest.join("vendor");
    let jolt_dir = root.join("Jolt");
    assert!(
        jolt_dir.join("Jolt.h").exists(),
        "Jolt source not found at {}. Restore the committed vendor directory.",
        root.display()
    );

    let deterministic = env::var_os("CARGO_FEATURE_CROSS_PLATFORM_DETERMINISTIC").is_some();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let msvc = env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc");

    let mut files = Vec::new();
    collect_cpp(&jolt_dir, &mut files);

    let profile = env::var_os("CARGO_FEATURE_PROFILE").is_some();
    if profile {
        // Instrument a build-directory copy; the pinned vendor tree stays pristine.
        let source = jolt_dir.join("Physics/PhysicsSystem.cpp");
        let mut text = std::fs::read_to_string(&source).expect("vendored PhysicsSystem.cpp");
        text = text.replace("\r\n", "\n");
        let marker = "void PhysicsSystem::ProcessBodyPair(ContactAllocator &ioContactAllocator, const BodyPair &inBodyPair)\n{";
        assert!(text.contains(marker), "profile marker changed upstream");
        text = text.replace(
            marker,
            &format!("{marker}\n JPH_PROFILE(\"AraNarrowphase\");"),
        );
        for line in text.clone().lines() {
            if line.contains("mBroadPhase->FindCollidingPairs(")
                || line.contains("mContactManager.AddContactConstraint(")
            {
                let label = if line.contains("FindCollidingPairs") {
                    "AraBroadphase"
                } else {
                    "AraContactSetup"
                };
                text = text.replace(line, &format!("{{ JPH_PROFILE(\"{label}\"); {line} }}"));
            }
        }
        let instrumented =
            PathBuf::from(env::var_os("OUT_DIR").expect("cargo OUT_DIR")).join("PhysicsSystem.cpp");
        std::fs::write(&instrumented, text).expect("write instrumented physics source");
        files.retain(|p| p != &source);
        files.push(instrumented);

        let source = jolt_dir.join("Physics/Constraints/ContactConstraintManager.cpp");
        let text = std::fs::read_to_string(&source)
            .expect("vendored contact manager")
            .replace("\r\n", "\n");
        let marker = "void ContactConstraintManager::TemplatedGetContactsFromCache(ContactAllocator &ioContactAllocator, Body &inBody1, Body &inBody2, const CachedBodyPair &inCachedBodyPair, CachedBodyPair &outCachedBodyPair)\n{";
        assert!(
            text.contains(marker),
            "cached contact profile marker changed upstream"
        );
        let text = text.replace(
            marker,
            &format!("{marker}\n JPH_PROFILE(\"AraContactSetup\");"),
        );
        let instrumented = PathBuf::from(env::var_os("OUT_DIR").expect("cargo OUT_DIR"))
            .join("ContactConstraintManager.cpp");
        std::fs::write(&instrumented, text).expect("write instrumented contact manager");
        files.retain(|p| p != &source);
        files.push(instrumented);
    }

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .include(&root)
        .include(manifest.join("shim"))
        .warnings(false)
        .files(&files)
        .files(
            [
                "world.cpp",
                "shapes.cpp",
                "bodies.cpp",
                "queries.cpp",
                "character.cpp",
                "vehicle.cpp",
                "joints.cpp",
            ]
            .map(|name| manifest.join("shim").join(name)),
        )
        // Physics must remain usable in a debug editor. Rust retains debug info;
        // native solver code is optimized identically in check/test/release.
        .opt_level(2)
        .debug(false)
        .define("NDEBUG", None);

    if env::var_os("CARGO_FEATURE_BENCH").is_some() {
        build.file(manifest.join("benches/native.cpp"));
    }
    if profile {
        build.define("JPH_EXTERNAL_PROFILE", None);
    }

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
        build.flag("/EHsc");
        build.flag("/Zc:__cplusplus");
        build.flag("/Gy");
        build.flag("/GS-");
    } else {
        build.flag("-fno-rtti");
        if deterministic {
            build.flag("-fno-fast-math");
            build.flag("-ffp-contract=off");
        }
    }

    build.compile("ara_jolt");

    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=benches");
    println!("cargo:rerun-if-changed={}", jolt_dir.display());
}
