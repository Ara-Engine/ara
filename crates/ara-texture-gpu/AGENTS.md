# ara-texture-gpu

Offline headless Vulkan block encoders for disposable import workers. No windows.
BC4/BC5/BC7 inputs are 16 packed RGBA8 u32 words per block; BC6H uses 32 words
(positive finite RGB half bits). One workgroup encodes one block. All output-
affecting shader math, reductions and tie breaking are integer and explicit.
Fast/Balanced/HQ shader preset IDs are 0/1/2, independent of CookPreset order.
`Encoder` implements ara-asset's BlockEncoder. Services are negotiated before cache
lookup; a failed GPU encode retries with a CPU key. ARA_TEXTURE_ENCODER=cpu disables
the optional device for controlled CPU measurements and CPU-only tests.
Normal-map imports retain qualified ISPC BC5 because GPU angular outliers regress;
the raw GPU BC5 API remains available for experiments/reference validation.
Balanced/HQ BC7 use eight specialized mode pipelines plus finalist selection and
reduction. Scratch is bounded to 1552 bytes/block and 65,535 blocks per submission.
Transfers use device-local storage and prefer cached coherent CPU readback memory.

Each codec owns its shader/includes and Rust reference/test module. Keep those
boundaries when delegating. No git stash: use plain WIP commits on your branch.

Run focused crate checks, tests and strict Clippy with CARGO_BUILD_JOBS=2.
Normal tests exercise independent decoders and integer references without Vulkan.
GPU measurements run through ara-dev gpu. Keep captures and generated reports
under target, with private ARA_CONFIG_DIR/ARA_CRASH_DIR; never use main's binaries.

Re-cook shaders/pipelines.json with ara-shader-cook; cooked artifacts are never
merged. Reference modules preserve their third-party table notices. Cross-vendor
integer determinism requires device execution evidence before claiming it tested.
