# Windows linker measurement

Measured on 2026-10-23, Windows x86-64, Ryzen 7 7800X3D / 32 GB, Rust 1.99.1
(LLVM 23.1.1), MSVC 14.50.35717. The normal dev/test profile has opt-level 1,
no debug info, and dependencies at opt-level 3. CARGO_BUILD_JOBS was 2.

Each target was linked three times per linker, alternating MSVC then rust-lld.
The shim times only the linker subprocess, including process startup. It excludes
dependency compilation and Rust code generation. The shared machine was busy;
all samples are retained below rather than treating the first sample as warm.

- ara-player / integration test pins: MSVC 5.404342, 1.834582, 3.157791 seconds;
  rust-lld 2.191566, 1.514501, 1.259639. Medians: 3.157791 → 1.514501 (52% less).
- ara-player / integration test csharp: MSVC 1.796777, 2.433226, 2.319319 seconds;
  rust-lld 1.323901, 1.507365, 1.147250. Medians: 2.319319 → 1.323901 (43% less).
- ara-editor / integration test editor: MSVC 7.334027, 4.103702, 9.276809 seconds;
  rust-lld 3.605074, 2.664623, 2.191474. Medians: 7.334027 → 2.664623 (64% less).

All 18 measured links succeeded. These targets include Vulkan, vendored SDL,
the .NET hosting path, Jolt and Opus dependencies. The tracked Windows target
configuration selects `rust-lld.exe`; rustc locates the toolchain's own linker
and the installed MSVC/Windows SDK import libraries. No PATH installation or new
prerequisite is needed. The full branch gate supplies the runtime correctness
check before this configuration is committed.

To reproduce, compile the `ara-dev` example `link_time` and use its executable as
the linker for only the selected final target with `cargo rustc`, keeping the
workspace's normal linker for dependencies:

```
cargo rustc -p ara-player --test pins -- -Clinker=<path-to-link_time.exe> -Clinker-flavor=msvc --cfg=link_sample_1
cargo rustc -p ara-player --test csharp -- -Clinker=<path-to-link_time.exe> -Clinker-flavor=msvc --cfg=link_sample_1
cargo rustc -p ara-editor --test editor -- -Clinker=<path-to-link_time.exe> -Clinker-flavor=msvc --cfg=link_sample_1
```

Set ARA_DEV_LINKER to the absolute real link.exe or toolchain rust-lld.exe path,
ARA_DEV_LINK_LOG to a TSV under target/, and ARA_DEV_LINK_LABEL to the target/sample
name. Change the sample cfg each time to force the final link without touching
tracked sources. For MSVC the shim forwards arguments unchanged; for rust-lld it
adds `-flavor link`. Use three or more samples and compare medians. To opt out of
the tracked linker for diagnosis, set CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER
to the installed MSVC link.exe path for that command.
