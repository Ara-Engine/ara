# ara-pack

Production content-addressed pack v2, independent of the runtime and exporter.
`build`/`build_directory` cook and chunk inputs; `Pack` validates borrowed bytes;
`MappedPack` maps immutable files; `diff` describes store delivery deltas.
`OwnedPack` owns immutable bytes, validates stored hashes once and lends views of
the same manifest. Individual reads still check decompressed identities; repeated
asset loads never rehash the whole pack or clone its manifest.

Run `cargo test -p ara-pack`, `cargo clippy -p ara-pack --all-targets`.
The `write` feature enables native zstd compression. `native-decode` selects the
vendored zstd reader, built through `cc` without CMake, in ara-player and its runtime
templates. Without it reading uses pure Rust. Both bound chunks to 16 MiB and keep
stored, raw and file hash checks; native decoding also limits the history window.
Source paths and timestamps never enter identities. JSON uses ara-json's writer.
Encryption is a deterrent, not DRM: clients necessarily possess the key.
Keys never enter manifests. Nonces derive from keyed chunk identity; immutable
chunk plaintext is compressed with the pinned format settings before encryption.
Mapped files must not be changed or truncated while mapped (unsafe open contract).
