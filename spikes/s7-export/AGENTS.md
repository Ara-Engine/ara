# AGENTS.md: S7 export spike (throwaway)

Proves Ara's shipping model on Windows, Linux and Android: a prebuilt Rust runtime
template + C# gameplay compiled ahead of time + a deterministic content pack.
Findings and numbers live in `RESULT.md`.

## Layout

- `src/` (crate `ara_runtime`): `pack.rs` (`.arapack` reader + writer behind `tools`),
  `abi.rs` (R10 host ABI), `runtime.rs` (content → spawn → bind C# → ticks → verify),
  `android.rs` (`ANativeActivity_onCreate`, no Java).
- `src/bin/ara-player.rs`: desktop runtime template. `src/bin/ara-s7/`: `pack`, `pack-info`,
  `doctor`, `export`, `bench-start`. `src/bin/zigcc.rs`: glibc-2.31 linker shim for Linux-from-Windows.
- `game/`: C# gameplay (one source, `AraLane=aot|r2r`). `mod/`: an IL mod (R2R lane only).
- `content/`: scene JSON + TGA, cooked into the pack. `android/`: manifest + bundle config.
- `scripts/`: WSL setup, glibc 2.31 check, the measurement series. `results/`: recorded runs.
- `tools/` (ignored): `bundletool.jar`, `zig-*/`. `out/`, `dist/` (ignored): build output.

## Commands

```sh
CARGO_BUILD_JOBS=4 cargo build --release --features tools --bins
./target/release/ara-s7 doctor [--platform windows|linux|android] [--json] [--devices]
./target/release/ara-s7 export --platform windows --lane aot|r2r [--clean] [--json]
./target/release/ara-s7 export --platform linux --lane aot|aot-cross|r2r
./target/release/ara-s7 export --platform android
cargo clippy --features tools --all-targets
```

## Gotchas

- Keep `game/Abi.cs` and `src/abi.rs` in sync by hand; both check `ABI_VERSION` and struct sizes.
- `Movement.Run` (C#) and `movement_reference` (Rust) must keep the same float op order:
  the verify stage demands a bit-exact checksum.
- NativeAOT on Windows needs `vswhere.exe` on PATH; `export` prepends it.
- Android: the APK must store `.so` and the pack uncompressed (`jar cfM0`, `zipalign -P 16`).
